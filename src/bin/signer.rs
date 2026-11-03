//! Release signing helper (minisign), used by the GitHub Actions workflow
//! and for local testing. Keeps the update format compatible with the Tauri
//! updater's `latest.json`: `signature` is the base64 of the minisign
//! signature file text.
//!
//! ```text
//! signer keygen <dir>                       writes commitwatch.key (secret) and commitwatch.pub
//! signer sign <file> [--key <path>]         writes <file>.sig; key from --key or $COMMITWATCH_SIGNING_KEY
//! signer manifest --version V --url U --sig <file.sig> --notes N --out latest.json
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use base64::Engine;
use minisign::{KeyPair, SecretKeyBox};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("keygen") => keygen(args.get(1).map(PathBuf::from)),
        Some("sign") => sign(&args[1..]),
        Some("manifest") => manifest(&args[1..]),
        _ => Err("usage: signer keygen <dir> | sign <file> [--key <path>] | manifest --version V --url U --sig <file.sig> [--notes N] --out <latest.json>".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("signer: {e}");
            ExitCode::FAILURE
        }
    }
}

fn keygen(dir: Option<PathBuf>) -> Result<(), String> {
    let dir = dir.ok_or("keygen needs a directory")?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let key_path = dir.join("commitwatch.key");
    let pub_path = dir.join("commitwatch.pub");
    if key_path.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite",
            key_path.display()
        ));
    }
    let KeyPair { pk, sk } = KeyPair::generate_unencrypted_keypair().map_err(|e| e.to_string())?;
    let sk_box = sk
        .to_box(Some("CommitWatch update signing key (keep secret)"))
        .map_err(|e| e.to_string())?;
    let pk_box = pk.to_box().map_err(|e| e.to_string())?;
    fs::write(&key_path, sk_box.into_string()).map_err(|e| e.to_string())?;
    fs::write(&pub_path, pk_box.into_string()).map_err(|e| e.to_string())?;
    println!("secret key: {}", key_path.display());
    println!("public key: {}", pub_path.display());
    Ok(())
}

fn load_secret_key(key_arg: Option<&str>) -> Result<minisign::SecretKey, String> {
    let text = match key_arg {
        Some(path) => fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?,
        None => std::env::var("COMMITWATCH_SIGNING_KEY")
            .map_err(|_| "no --key and COMMITWATCH_SIGNING_KEY is not set".to_string())?,
    };
    // Keys from `keygen` are unencrypted; a password-protected key (made with
    // the minisign CLI) is also accepted via COMMITWATCH_SIGNING_KEY_PASSWORD.
    let unencrypted = SecretKeyBox::from_string(&text).map_err(|e| e.to_string())?;
    if let Ok(sk) = minisign::SecretKey::from_unencrypted_box(unencrypted) {
        return Ok(sk);
    }
    let password = std::env::var("COMMITWATCH_SIGNING_KEY_PASSWORD").ok();
    SecretKeyBox::from_string(&text)
        .map_err(|e| e.to_string())?
        .into_secret_key(password)
        .map_err(|e| e.to_string())
}

fn sign(args: &[String]) -> Result<(), String> {
    let file = args.first().ok_or("sign needs a file")?;
    let key_arg = args
        .iter()
        .position(|a| a == "--key")
        .and_then(|i| args.get(i + 1));
    let sk = load_secret_key(key_arg.map(String::as_str))?;
    let data = fs::read(file).map_err(|e| format!("read {file}: {e}"))?;
    let name = Path::new(file)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let trusted = format!("timestamp:{}\tfile:{name}", chrono::Utc::now().timestamp());
    let sig = minisign::sign(
        None,
        &sk,
        &data[..],
        Some(&trusted),
        Some("signature from CommitWatch signer"),
    )
    .map_err(|e| e.to_string())?;
    let sig_path = format!("{file}.sig");
    let text = sig.into_string();
    fs::write(&sig_path, &text).map_err(|e| e.to_string())?;
    println!("{sig_path}");
    println!(
        "{}",
        base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
    );
    Ok(())
}

fn manifest(args: &[String]) -> Result<(), String> {
    let get = |flag: &str| -> Option<&String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
    };
    let version = get("--version").ok_or("--version is required")?;
    let url = get("--url").ok_or("--url is required")?;
    let sig_file = get("--sig").ok_or("--sig is required")?;
    let out = get("--out").ok_or("--out is required")?;
    let notes = get("--notes").cloned().unwrap_or_default();
    let sig_text = fs::read_to_string(sig_file).map_err(|e| format!("read {sig_file}: {e}"))?;
    let signature = base64::engine::general_purpose::STANDARD.encode(sig_text.as_bytes());
    let json = serde_json::json!({
        "version": version.trim_start_matches('v'),
        "notes": notes,
        "pub_date": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "platforms": {
            "windows-x86_64": { "signature": signature, "url": url }
        }
    });
    let text = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    fs::write(out, text).map_err(|e| e.to_string())?;
    println!("{out}");
    Ok(())
}
