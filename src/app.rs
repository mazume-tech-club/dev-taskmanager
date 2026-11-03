//! The window process: numbers and graphs for the current state, a Top
//! Processes list, a CPU view and a settings tab. Reads the resident
//! process's shared memory once a second. Closing the window ends this
//! process; the resident keeps monitoring.

use crate::alert::AlertLevel;
use crate::autostart;
use crate::config::{self, Config};
use crate::icon;
use crate::metrics::fmt_gib;
use crate::pawnio;
use crate::sample::{Sample, SensorState};
use crate::shm;
use crate::update;
use eframe::egui::{self, Color32, RichText, Ui};
use egui_plot::{Legend, Line, Plot, PlotPoints};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

pub const NORMAL_SIZE: [f32; 2] = [780.0, 600.0];
pub const COMPACT_SIZE: [f32; 2] = [340.0, 150.0];

/// BIZ UDGothic (SIL OFL 1.1), subset by tools/subset_font.py.
const UI_FONT: &[u8] = include_bytes!("../assets/fonts/BIZUDGothic-Regular-subset.ttf");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Processes,
    Cpu,
    Settings,
}

pub struct App {
    reader: Option<shm::Reader>,
    history: Vec<Sample>,
    last_read: Instant,
    cfg: Config,
    draft: Config,
    draft_autostart: bool,
    tab: Tab,
    compact: bool,
    status: Option<String>,
    update_status: Option<String>,
    update_rx: Option<Receiver<String>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: Config) -> Self {
        install_font(&cc.egui_ctx);
        Self {
            reader: shm::Reader::open().ok().flatten(),
            history: Vec::new(),
            last_read: Instant::now() - Duration::from_secs(10),
            draft: cfg.clone(),
            draft_autostart: autostart::is_enabled(),
            cfg,
            tab: Tab::Overview,
            compact: false,
            status: None,
            update_status: None,
            update_rx: None,
        }
    }

    fn refresh(&mut self) {
        if self.last_read.elapsed() < Duration::from_millis(900) {
            return;
        }
        self.last_read = Instant::now();
        if self.reader.is_none() {
            self.reader = shm::Reader::open().ok().flatten();
        }
        if let Some(r) = &self.reader {
            self.history = r.read();
        }
    }

    fn set_compact(&mut self, ctx: &egui::Context, compact: bool) {
        self.compact = compact;
        let (size, level) = if compact {
            (COMPACT_SIZE, egui::WindowLevel::AlwaysOnTop)
        } else {
            (NORMAL_SIZE, egui::WindowLevel::Normal)
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size.into()));
    }

    fn level_of(&self, s: &Sample) -> AlertLevel {
        AlertLevel::from_commit_usage(s.commit_usage(), &self.cfg.alert).max(
            AlertLevel::from_temp_margin(s.temp_margin_c, &self.cfg.alert),
        )
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        ctx.request_repaint_after(Duration::from_secs(1));
        self.refresh();

        let latest = self.history.last().cloned();
        let n = self.history.len();
        let mem: Vec<[f64; 3]> = self
            .history
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let t = -((n - 1 - i) as f64);
                let avail = if x.memory.physical_total == 0 {
                    0.0
                } else {
                    x.memory.physical_available as f64 / x.memory.physical_total as f64
                };
                [t, x.commit_usage() * 100.0, avail * 100.0]
            })
            .collect();
        let cpu: Vec<[f64; 2]> = self
            .history
            .iter()
            .enumerate()
            .map(|(i, x)| [-((n - 1 - i) as f64), x.cpu_total * 100.0])
            .collect();

        if self.compact {
            egui::CentralPanel::default().show(root, |ui| {
                self.compact_view(ui, &ctx, latest.as_ref(), &mem);
            });
            return;
        }

        egui::Panel::top("tabs").show(root, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Overview, "概要");
                ui.selectable_value(&mut self.tab, Tab::Processes, "プロセス");
                ui.selectable_value(&mut self.tab, Tab::Cpu, "CPU");
                ui.selectable_value(&mut self.tab, Tab::Settings, "設定");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("コンパクト").clicked() {
                        self.set_compact(&ctx, true);
                    }
                    if ui.button("トレイへ").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        });
        egui::CentralPanel::default().show(root, |ui| {
            if self.reader.is_none() {
                ui.colored_label(Color32::YELLOW, "常駐プロセスに接続中…（トレイの CommitWatch が起動していることを確認してください）");
            }
            match self.tab {
                Tab::Overview => self.overview(ui, latest.as_ref(), &mem, &cpu),
                Tab::Processes => self.processes(ui, latest.as_ref()),
                Tab::Cpu => self.cpu(ui, latest.as_ref()),
                Tab::Settings => self.settings(ui),
            }
        });
    }
}

impl App {
    fn compact_view(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        latest: Option<&Sample>,
        mem: &[[f64; 3]],
    ) {
        ui.horizontal(|ui| {
            if ui.small_button("通常").clicked() {
                self.set_compact(ctx, false);
            }
            if ui.small_button("トレイへ").clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        let Some(s) = latest else {
            ui.label("取得中…");
            return;
        };
        let level = self.level_of(s);
        let [r, g, b] = icon::colour(level);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Commit {:.0}%", s.commit_usage() * 100.0))
                    .size(28.0)
                    .color(Color32::from_rgb(r, g, b)),
            );
            ui.vertical(|ui| {
                ui.label(format!(
                    "{} / {}",
                    fmt_gib(s.memory.commit_charge),
                    fmt_gib(s.memory.commit_limit)
                ));
                ui.label(format!(
                    "空き RAM {}  CPU {:.0}%",
                    fmt_gib(s.memory.physical_available),
                    s.cpu_total * 100.0
                ));
            });
        });
        Plot::new("compact")
            .height(50.0)
            .include_y(0.0)
            .include_y(100.0)
            .show_axes([false, false])
            .show_grid(false)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .show(ui, |p| {
                p.line(Line::new(
                    "Commit %",
                    PlotPoints::from_iter(mem.iter().map(|m| [m[0], m[1]])),
                ));
            });
    }

    fn overview(&self, ui: &mut Ui, latest: Option<&Sample>, mem: &[[f64; 3]], cpu: &[[f64; 2]]) {
        let Some(s) = latest else {
            ui.label("取得中…");
            return;
        };
        let level = self.level_of(s);
        let [r, g, b] = icon::colour(level);
        let colour = Color32::from_rgb(r, g, b);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Commit Usage {:.1}%", s.commit_usage() * 100.0))
                    .size(30.0)
                    .color(colour),
            );
            ui.label(RichText::new(level.label_ja()).size(20.0).color(colour));
        });
        ui.add(
            egui::ProgressBar::new(s.commit_usage() as f32)
                .fill(colour)
                .text(format!(
                    "Commit Charge {} / Commit Limit {}",
                    fmt_gib(s.memory.commit_charge),
                    fmt_gib(s.memory.commit_limit)
                )),
        );
        ui.add_space(6.0);
        egui::Grid::new("numbers")
            .num_columns(4)
            .spacing([24.0, 4.0])
            .show(ui, |ui| {
                ui.label("物理メモリ");
                ui.label(fmt_gib(s.memory.physical_total));
                ui.label("利用可能（Available）");
                ui.label(fmt_gib(s.memory.physical_available));
                ui.end_row();
                ui.label("  空き（Free）");
                ui.label(fmt_gib(s.memory.free));
                ui.label("  スタンバイ");
                ui.label(fmt_gib(s.memory.standby));
                ui.end_row();
                ui.label("ページファイル");
                ui.label(format!(
                    "{} / {}{}",
                    fmt_gib(s.memory.page_file_in_use()),
                    fmt_gib(s.memory.page_file_total()),
                    if s.memory.page_file_automatic {
                        "（自動）"
                    } else {
                        "（手動）"
                    }
                ));
                ui.label("変更済み（Modified）");
                ui.label(fmt_gib(s.memory.modified));
                ui.end_row();
                ui.label("VM Memory（WSL2 / Docker）");
                ui.label(fmt_gib(s.vm_memory));
                ui.label("Kernel Commit（カーネル等）");
                ui.label(fmt_gib(s.kernel_commit));
                ui.end_row();
                ui.label("CPU Load");
                ui.label(format!("{:.1}%", s.cpu_total * 100.0));
                ui.label("温度ゾーン（ACPI）");
                ui.label(
                    s.thermal_zone_c
                        .map(|t| format!("{t:.1} ℃"))
                        .unwrap_or_else(|| "なし".into()),
                );
                ui.end_row();
                ui.label("Package Temperature");
                ui.label(match (s.package_temp_c, s.sensor_state) {
                    (Some(t), _) => {
                        format!("{t:.0} ℃（TjMax {:.0} ℃）", s.tj_max_c.unwrap_or(100.0))
                    }
                    (None, st) => st.label_ja().to_string(),
                });
                ui.label("パッケージ電力");
                ui.label(
                    s.package_power_w
                        .map(|w| format!("{w:.1} W"))
                        .unwrap_or_else(|| "-".into()),
                );
                ui.end_row();
            });
        ui.add_space(8.0);
        ui.label("直近 10 分（%）");
        let span = -(self.cfg.general.history_secs as f64);
        Plot::new("memory")
            .height(180.0)
            .include_y(0.0)
            .include_y(100.0)
            .include_x(span)
            .include_x(0.0)
            .legend(Legend::default())
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .show(ui, |p| {
                p.line(Line::new(
                    "Commit Usage",
                    PlotPoints::from_iter(mem.iter().map(|m| [m[0], m[1]])),
                ));
                p.line(Line::new(
                    "Available RAM / 物理メモリ",
                    PlotPoints::from_iter(mem.iter().map(|m| [m[0], m[2]])),
                ));
            });
        Plot::new("cpu")
            .height(100.0)
            .include_y(0.0)
            .include_y(100.0)
            .include_x(span)
            .include_x(0.0)
            .legend(Legend::default())
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .show(ui, |p| {
                p.line(Line::new(
                    "CPU Load",
                    PlotPoints::from_iter(cpu.iter().copied()),
                ));
            });
    }

    fn processes(&self, ui: &mut Ui, latest: Option<&Sample>) {
        let Some(s) = latest else {
            ui.label("取得中…");
            return;
        };
        ui.label("Process Commit 上位（5 秒ごとに更新）");
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("procs")
                .num_columns(4)
                .striped(true)
                .spacing([16.0, 2.0])
                .show(ui, |ui| {
                    ui.strong("PID");
                    ui.strong("名前");
                    ui.strong("Process Commit");
                    ui.strong("ワーキングセット");
                    ui.end_row();
                    for p in &s.top_processes {
                        ui.label(p.pid.to_string());
                        ui.label(if p.is_vm() {
                            format!("{}  (VM Memory)", p.name)
                        } else {
                            p.name.clone()
                        });
                        ui.label(fmt_gib(p.commit));
                        ui.label(fmt_gib(p.working_set));
                        ui.end_row();
                    }
                });
        });
    }

    fn cpu(&self, ui: &mut Ui, latest: Option<&Sample>) {
        let Some(s) = latest else {
            ui.label("取得中…");
            return;
        };
        ui.label(format!("CPU Load 全体 {:.1}%", s.cpu_total * 100.0));
        ui.add_space(4.0);
        for (i, load) in s.cpu_per_logical.iter().enumerate() {
            let mhz = s.clock_mhz.get(i).copied().unwrap_or(0.0);
            ui.add(egui::ProgressBar::new(*load as f32).text(format!(
                "CPU {i:<2}  {:>5.1}%   {mhz:>5.0} MHz",
                load * 100.0
            )));
        }
        ui.add_space(8.0);
        ui.label(format!(
            "温度ゾーン（ACPI、マザーボードの値）: {}",
            s.thermal_zone_c
                .map(|t| format!("{t:.1} ℃"))
                .unwrap_or_else(|| "なし".into())
        ));
        ui.add_space(8.0);
        ui.heading("CPU センサー（Sensor Provider: PawnIO）");
        match s.sensor_state {
            SensorState::Available => {
                let tj = s.tj_max_c.unwrap_or(100.0);
                egui::Grid::new("sensors")
                    .num_columns(4)
                    .spacing([24.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Package Temperature");
                        ui.label(
                            s.package_temp_c
                                .map(|t| format!("{t:.0} ℃"))
                                .unwrap_or_else(|| "-".into()),
                        );
                        ui.label("TjMax");
                        ui.label(format!("{tj:.0} ℃"));
                        ui.end_row();
                        ui.label("パッケージ電力");
                        ui.label(
                            s.package_power_w
                                .map(|w| format!("{w:.1} W"))
                                .unwrap_or_else(|| "-".into()),
                        );
                        ui.label("TDP");
                        ui.label(
                            s.tdp_w
                                .map(|w| format!("{w:.0} W"))
                                .unwrap_or_else(|| "-".into()),
                        );
                        ui.end_row();
                        ui.label("VID");
                        ui.label(
                            s.vid_v
                                .map(|v| format!("{v:.3} V"))
                                .unwrap_or_else(|| "-".into()),
                        );
                        ui.label("TjMax までの余裕");
                        ui.label(
                            s.temp_margin_c
                                .map(|m| format!("{m:.0} ℃"))
                                .unwrap_or_else(|| "-".into()),
                        );
                        ui.end_row();
                    });
                for (i, t) in s.core_temps_c.iter().enumerate() {
                    ui.add(
                        egui::ProgressBar::new((t / tj) as f32)
                            .text(format!("Core {i:<2}  {t:>4.0} ℃")),
                    );
                }
            }
            SensorState::Disabled => {
                ui.label("設定で無効になっています（設定タブ → CPU センサー）。");
            }
            SensorState::NotInstalled => {
                ui.label("PawnIO ドライバが未インストールです。トレイメニューの「PawnIO をインストール」からセットアップをダウンロードし、インストール後にトレイメニューの「管理者権限で再起動」を選んでください。");
                if ui.link("PawnIO セットアップをダウンロード").clicked() {
                    let _ = std::process::Command::new("cmd")
                        .args(["/C", "start", "", pawnio::INSTALLER_URL])
                        .spawn();
                }
            }
            SensorState::NotElevated => {
                ui.label("温度の読み取りには管理者権限が必要です。トレイメニューの「管理者権限で再起動」を選んでください。");
            }
            SensorState::Failed => {
                ui.label("センサーの読み取りに失敗しました（Intel 以外の CPU、または対応外）。");
            }
        }
    }

    fn settings(&mut self, ui: &mut Ui) {
        let d = &mut self.draft;
        ui.heading("しきい値");
        let mut warn = d.alert.warning_commit_usage * 100.0;
        let mut crit = d.alert.critical_commit_usage * 100.0;
        ui.add(
            egui::Slider::new(&mut warn, 50.0..=100.0)
                .suffix("%")
                .text("警告 Commit Usage"),
        );
        ui.add(
            egui::Slider::new(&mut crit, 50.0..=100.0)
                .suffix("%")
                .text("危険 Commit Usage"),
        );
        d.alert.warning_commit_usage = warn / 100.0;
        d.alert.critical_commit_usage = crit.max(warn) / 100.0;
        ui.add(
            egui::Slider::new(&mut d.alert.warning_temp_margin_c, 0.0..=40.0)
                .suffix(" ℃")
                .text("警告 TjMax までの余裕"),
        );
        ui.add(
            egui::Slider::new(&mut d.alert.critical_temp_margin_c, 0.0..=40.0)
                .suffix(" ℃")
                .text("危険 TjMax までの余裕"),
        );

        ui.add_space(8.0);
        ui.heading("ログ");
        ui.checkbox(&mut d.log.enabled, "1 分ごとの CSV ログを書く");
        ui.add(
            egui::Slider::new(&mut d.log.retention_days, 1..=365)
                .suffix(" 日")
                .text("保持日数"),
        );
        ui.add(
            egui::Slider::new(&mut d.general.top_processes, 1..=20)
                .text("記録する Top Processes 数"),
        );

        ui.add_space(8.0);
        ui.heading("更新");
        ui.checkbox(&mut d.update.enabled, "自動で更新を確認する");
        ui.add(
            egui::Slider::new(&mut d.update.check_interval_hours, 1..=168)
                .suffix(" 時間")
                .text("確認間隔"),
        );
        ui.label(format!("現在のバージョン: v{}", update::CURRENT_VERSION));
        if let Some(st) = update::staged() {
            ui.colored_label(
                Color32::from_rgb(46, 160, 67),
                format!(
                    "v{} をダウンロード済み（署名確認済み）。トレイアイコンの右クリック →「更新して再起動」で適用します。",
                    st.version
                ),
            );
            if !st.notes.is_empty() {
                ui.label(st.notes);
            }
        }
        if let Some(rx) = &self.update_rx
            && let Ok(msg) = rx.try_recv()
        {
            self.update_status = Some(msg);
            self.update_rx = None;
        }
        ui.horizontal(|ui| {
            let busy = self.update_rx.is_some();
            if ui
                .add_enabled(!busy, egui::Button::new("今すぐ確認"))
                .clicked()
            {
                let endpoint = update::endpoint(&self.cfg.update);
                let (tx, rx) = std::sync::mpsc::channel();
                self.update_rx = Some(rx);
                self.update_status = Some("確認中…".into());
                std::thread::spawn(move || {
                    let msg = match update::check_and_stage(&endpoint) {
                        Ok(update::Outcome::UpToDate) => "最新版です。".to_string(),
                        Ok(update::Outcome::Staged(st)) => {
                            format!("v{} を準備しました。", st.version)
                        }
                        Err(e) => format!("確認に失敗: {e}"),
                    };
                    let _ = tx.send(msg);
                });
            }
            if let Some(st) = &self.update_status {
                ui.label(st);
            }
        });

        ui.add_space(8.0);
        ui.heading("CPU センサー");
        ui.checkbox(
            &mut d.sensors.enabled,
            "PawnIO 経由でコア温度・電力を読む（管理者権限が必要。変更は常駐の再起動後に反映）",
        );

        ui.add_space(8.0);
        ui.heading("起動");
        ui.checkbox(
            &mut self.draft_autostart,
            "Windows 起動時に開始する（トレイに隠れて起動）",
        );

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("保存").clicked() {
                self.draft.general.autostart = self.draft_autostart;
                let mut msg = String::new();
                match self.draft.save() {
                    Ok(()) => {
                        self.cfg = self.draft.clone();
                        msg.push_str("保存しました。常駐側は数秒以内に反映します。");
                    }
                    Err(e) => msg.push_str(&format!("保存に失敗: {e}")),
                }
                if autostart::is_enabled() != self.draft_autostart {
                    match autostart::set(self.draft_autostart) {
                        Ok(()) => msg.push_str(" 自動起動を更新しました。"),
                        Err(e) => msg.push_str(&format!(" 自動起動の更新に失敗: {e}")),
                    }
                }
                self.status = Some(msg);
            }
            if ui.button("ログフォルダを開く").clicked()
                && let Ok(dir) = config::log_dir()
            {
                let _ = std::process::Command::new("explorer.exe").arg(dir).spawn();
            }
            if ui.button("設定ファイルの場所").clicked()
                && let Ok(path) = config::config_path()
            {
                self.status = Some(path.display().to_string());
            }
        });
        if let Some(st) = &self.status {
            ui.label(st);
        }
    }
}

/// egui's bundled fonts have no CJK glyphs; the embedded subset font goes
/// first so Japanese and Latin text share one face.
fn install_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("ud".into(), Arc::new(egui::FontData::from_static(UI_FONT)));
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "ud".into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("ud".into());
    ctx.set_fonts(fonts);
}

pub fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: icon::rgba(64, AlertLevel::Normal, 0.6),
        width: 64,
        height: 64,
    }
}
