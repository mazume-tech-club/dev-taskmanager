//! Embeds the application icon and version info into commitwatch.exe so
//! Explorer, the taskbar and the file properties show them.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/commitwatch.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/commitwatch.ico");
        res.set("ProductName", "CommitWatch");
        res.set(
            "FileDescription",
            "CommitWatch - Windows memory commit monitor",
        );
        if let Err(e) = res.compile() {
            println!("cargo:warning=resource compile failed: {e}");
        }
    }
}
