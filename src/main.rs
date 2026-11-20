use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use eframe::egui;
use enigo::{Direction::{Click, Press, Release}, Enigo, Key, Keyboard, Settings};

fn physical_key(ch: char) -> Option<(Key, bool)> {
    let key = match ch.to_ascii_lowercase() {
        'a' => Key::A, 'b' => Key::B, 'c' => Key::C, 'd' => Key::D,
        'e' => Key::E, 'f' => Key::F, 'g' => Key::G, 'h' => Key::H,
        'i' => Key::I, 'j' => Key::J, 'k' => Key::K, 'l' => Key::L,
        'm' => Key::M, 'n' => Key::N, 'o' => Key::O, 'p' => Key::P,
        'q' => Key::Q, 'r' => Key::R, 's' => Key::S, 't' => Key::T,
        'u' => Key::U, 'v' => Key::V, 'w' => Key::W, 'x' => Key::X,
        'y' => Key::Y, 'z' => Key::Z,
        '0' => Key::Num0, '1' | '!' => Key::Num1,
        '2' | '"' => Key::Num2, '3' | '#' => Key::Num3,
        '4' | '$' => Key::Num4, '5' | '%' => Key::Num5,
        '6' | '&' => Key::Num6, '7' | '\'' => Key::Num7,
        '8' | '(' => Key::Num8, '9' | ')' => Key::Num9,
        // JP106: punctuation keys differ from the US layout.
        '-' | '=' => Key::OEMMinus,
        '^' | '~' => Key::OEM7,
        '\\' | '|' => Key::OEM5,
        '@' | '`' => Key::OEM3,
        '[' | '{' => Key::OEM4, ']' | '}' => Key::OEM6,
        ';' | '+' => Key::OEMPlus, ':' | '*' => Key::OEM1,
        ',' | '<' => Key::OEMComma, '.' | '>' => Key::OEMPeriod,
        '/' | '?' => Key::OEM2, '_' => Key::OEM102,
        ' ' => Key::Space,
        _ => return None,
    };
    Some((key, ch.is_ascii_uppercase() || "!\"#$%&'()=~|@{}+*:<>?_".contains(ch)))
}

fn type_char(enigo: &mut Enigo, ch: char) -> enigo::InputResult<()> {
    // noVNC listens for physical keydown/keyup events. Unicode injection can
    // bypass those events, so use the US keyboard scan-code path for ASCII.
    let Some((key, shifted)) = physical_key(ch) else {
        return enigo.text(&ch.to_string());
    };

    if shifted { enigo.key(Key::LShift, Press)?; }
    let result = enigo.key(key, Click);
    if shifted {
        // Always release Shift, even if the key event failed.
        let release_result = enigo.key(Key::LShift, Release);
        result.and(release_result)
    } else {
        result
    }
}


fn configure_fonts(ctx: &egui::Context) {
    let candidates: &[&str] = &[
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\YuGothR.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
    ];

    let Some((path, data)) = candidates
        .iter()
        .find_map(|path| fs::read(path).ok().map(|data| (*path, data)))
    else {
        eprintln!("Japanese font not found; using egui default fonts");
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    let font_name = "system_japanese".to_owned();

    fonts.font_data.insert(
        font_name.clone(),
        Arc::new(egui::FontData::from_owned(data)),
    );

    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, font_name.clone());
    }

    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        family.push(font_name);
    }

    ctx.set_fonts(fonts);
    eprintln!("Loaded Japanese font: {path}");
}

struct AutoTyperApp {
    file_path: Option<PathBuf>,
    delay_ms: u64,
    countdown_secs: u64,
    status: String,
    running: bool,
    stop_flag: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    total: usize,
}

impl Default for AutoTyperApp {
    fn default() -> Self {
        Self {
            file_path: None,
            delay_ms: 1,
            countdown_secs: 5,
            status: "テキストファイルを選択してください".into(),
            running: false,
            stop_flag: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(AtomicUsize::new(0)),
            total: 0,
        }
    }
}

impl AutoTyperApp {
    fn start_typing(&mut self, ctx: egui::Context) {
        let Some(path) = self.file_path.clone() else {
            self.status = "ファイルが選択されていません".into();
            return;
        };

        let bytes = match fs::read(&path) {
            Ok(v) => v,
            Err(e) => {
                self.status = format!("読み込みエラー: {e}");
                return;
            }
        };

        let text = match String::from_utf8(bytes) {
            Ok(s) => s.replace("\r\n", "\n").replace('\r', "\n"),
            Err(e) => {
                self.status = format!("UTF-8ではありません: {e}");
                return;
            }
        };

        let chars: Vec<char> = text.chars().collect();
        self.total = chars.len();
        self.progress.store(0, Ordering::Relaxed);
        self.stop_flag.store(false, Ordering::Relaxed);
        self.running = true;
        self.status = format!(
            "{}秒後に入力開始。入力先のウィンドウへフォーカスしてください",
            self.countdown_secs
        );

        let stop_flag = Arc::clone(&self.stop_flag);
        let progress = Arc::clone(&self.progress);
        let delay = Duration::from_millis(self.delay_ms);
        let countdown = self.countdown_secs;

        thread::spawn(move || {
            for _ in 0..countdown {
                if stop_flag.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_secs(1));
            }

            let mut enigo = match Enigo::new(&Settings::default()) {
                Ok(v) => v,
                Err(_) => return,
            };

            for (i, ch) in chars.into_iter().enumerate() {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }

                let result = match ch {
                    '\n' => enigo.key(Key::Return, Click),
                    '\t' => enigo.key(Key::Tab, Click),
                    _ => type_char(&mut enigo, ch),
                };

                if result.is_err() {
                    stop_flag.store(true, Ordering::Relaxed);
                    break;
                }

                progress.store(i + 1, Ordering::Relaxed);
                ctx.request_repaint();

                if !delay.is_zero() {
                    thread::sleep(delay);
                }
            }

            ctx.request_repaint();
        });
    }
}

impl eframe::App for AutoTyperApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let done = self.progress.load(Ordering::Relaxed);

        if self.running {
        if self.stop_flag.load(Ordering::Relaxed) {
                self.running = false;
                self.status = format!("停止しました ({done}/{})", self.total);
        } else if self.total > 0 && done >= self.total {
                self.running = false;
                self.status = format!("完了: {}文字入力しました", self.total);
        }
        }

        ui.heading("Auto Typer");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.running, egui::Button::new("ファイルを選択"))
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Text / HTML", &["txt", "html", "htm", "js", "css"])
                        .pick_file()
                    {
                        match fs::metadata(&path) {
                            Ok(meta) => {
                                self.status = format!("選択: {} bytes", meta.len());
                                self.file_path = Some(path);
                            }
                            Err(e) => self.status = format!("ファイル情報取得エラー: {e}"),
                        }
                    }
                }

                if let Some(path) = &self.file_path {
                    ui.label(path.display().to_string());
                }
        });

        ui.add_space(12.0);

        ui.add_enabled_ui(!self.running, |ui| {
                ui.horizontal(|ui| {
                    ui.label("1文字ごとの待機:");
                    ui.add(egui::DragValue::new(&mut self.delay_ms).range(0..=1000).suffix(" ms"));
                });

                ui.horizontal(|ui| {
                    ui.label("開始まで:");
                    ui.add(
                        egui::DragValue::new(&mut self.countdown_secs)
                            .range(0..=30)
                            .suffix(" 秒"),
                    );
                });
        });

        ui.add_space(12.0);

        ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.running && self.file_path.is_some(),
                        egui::Button::new("入力開始"),
                    )
                    .clicked()
                {
                    self.start_typing(ui.ctx().clone());
                }

                if ui
                    .add_enabled(self.running, egui::Button::new("停止"))
                    .clicked()
                {
                    self.stop_flag.store(true, Ordering::Relaxed);
                }
        });

        ui.add_space(12.0);

        let fraction = if self.total == 0 {
                0.0
        } else {
                done as f32 / self.total as f32
        };
        ui.add(
                egui::ProgressBar::new(fraction)
                    .show_percentage()
                    .text(format!("{done} / {} chars", self.total)),
        );

        ui.add_space(8.0);
        ui.label(&self.status);

        ui.separator();
        ui.small(
                "開始後、カウントダウン中に入力先へフォーカスしてください。\n\
                 改行は Enter、Tab は Tab、それ以外は文字入力として送信します。",
        );
        if self.running {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }
}
fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([620.0, 300.0])
            .with_min_inner_size([480.0, 260.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Auto Typer",
        options,
        Box::new(|cc| {
            configure_fonts(&cc.egui_ctx);
            Ok(Box::<AutoTyperApp>::default())
        }),
    )
}
