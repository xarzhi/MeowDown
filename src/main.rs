//! MeowDown — a Typora-style WYSIWYG Markdown editor written with GPUI Kit.

// A console-subsystem executable drags a black console window along with it
// when it is started from a shortcut, from Explorer, or by the installer — a
// document editor should arrive as one window, not two. The debug build keeps
// the console, because that is where `println!` and `dbg!` go.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assoc;
mod editor;
mod inline;
mod math;
mod model;
mod remote;
mod settings;
mod shellops;

use gpui_kit::base::input::CursorBlink;
use gpui_kit::*;
use std::borrow::Cow;

/// The code face, embedded rather than hoped for.
///
/// JetBrains Mono is the family IntelliJ IDEA sets its editor in: a generous
/// x-height, unambiguous `1 l I`, and shapes cut to sit on a pixel grid at the
/// sizes a document is read at. It ships with MeowDown because GPUI panics on
/// the first line it lays out in a family the machine does not have — a reader
/// without the font installed would lose the window over a code fence. Six
/// faces, one family: regular (what inline chips and code fences use), light,
/// medium, and the bold/italic trio, so no weight is ever synthesised.
///
/// Licensed under the SIL Open Font License; see `assets/fonts/OFL.txt`.
const CODE_FACES: [&[u8]; 6] = [
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Light.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-BoldItalic.ttf"),
];

/// Whether the bundled face is in the text system. A family is resolved the
/// first time a line is shaped, so the answer is fixed for the run — and
/// reading it back is how `mono_font` knows whether the name is safe to ask
/// for.
static CODE_FACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Hands the embedded faces to the text system. Anything that lays out text
/// has to come after this: a family that is asked for before it is registered
/// is a family that does not exist.
fn install_fonts(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = CODE_FACES
        .iter()
        .map(|face| Cow::Borrowed(*face))
        .collect();
    let loaded = cx.text_system().add_fonts(fonts).is_ok();
    if !loaded {
        eprintln!("MeowDown: could not load the bundled code font, falling back");
    }
    let _ = CODE_FACE.set(loaded);
}

/// Whether the bundled code face is available, so the editor knows whether it
/// may name it. False before `install_fonts` runs, and false in tests, which
/// never open a window.
pub(crate) fn code_face_loaded() -> &'static bool {
    static NO: bool = false;
    CODE_FACE.get().unwrap_or(&NO)
}

/// What a command line asked for. The default is the window; the rest exist so
/// an installer — or a user who just wants the association — can say so without
/// a dialog.
enum Mode {
    Open,
    Register,
    Unregister,
}

/// Takes the `.md` association the first time a fresh copy runs, so the program
/// a reader just unpacked is the one that opens a document — without a trip to
/// Settings for the half of it Windows lets a program do for itself.
///
/// Three runs are left alone: one started from a build tree, because a
/// `target\debug` binary must never capture a reader's file associations; one
/// after the first, because re-registering on every start would quietly undo a
/// "取消关联" the user asked for; and one that already is the default, where
/// there is nothing to claim.
fn claim_association_once() {
    if std::env::var_os("MEOWDOWN_NO_ASSOC").is_some() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if is_build_tree(&exe) || assoc::has_asked() || assoc::is_default() {
        return;
    }
    let _ = assoc::register(&exe);
}

/// Whether this binary was built into a `target` directory, which is to say
/// whether it is a build rather than an installation.
fn is_build_tree(exe: &std::path::Path) -> bool {
    exe.components()
        .any(|part| part.as_os_str().eq_ignore_ascii_case("target"))
}

fn main() {
    let mut target = None;
    let mut mode = Mode::Open;
    // Opening a file from Explorer passes its path as the first argument.
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--register" | "/register" => mode = Mode::Register,
            "--unregister" | "/unregister" => mode = Mode::Unregister,
            "--version" | "-V" => {
                println!("MeowDown {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--help" | "-h" => {
                println!(
                    "MeowDown {}\n\
                     用法：meowdown [文件.md]\n\
                     \x20 --register      设为 .md 的默认应用（只写当前用户注册表）\n\
                     \x20 --unregister    取消关联\n\
                     \x20 --version       打印版本号",
                    env!("CARGO_PKG_VERSION")
                );
                return;
            }
            _ => {
                let path = std::path::PathBuf::from(&arg);
                if (path.is_file() || path.is_dir()) && target.is_none() {
                    target = Some(path);
                }
            }
        }
    }

    if let Mode::Register = mode {
        match std::env::current_exe()
            .map_err(|err| err.to_string())
            .and_then(|exe| assoc::register(&exe))
        {
            Ok(()) => println!("{}", assoc::status()),
            Err(err) => {
                eprintln!("关联失败：{err}");
                std::process::exit(1);
            }
        }
        return;
    }
    if let Mode::Unregister = mode {
        match assoc::unregister() {
            Ok(()) => println!("已取消 .md 关联"),
            Err(err) => {
                eprintln!("取消失败：{err}");
                std::process::exit(1);
            }
        }
        return;
    }

    claim_association_once();

    application()
        .with_assets(assets::Assets)
        .run(move |cx| {
            init(cx);
            // A caret is painted only while the blink cursor reports itself
            // visible, and that flag starts out false — it is a timer that has
            // to be ticked. Held steady, the caret is simply always there,
            // which is what a document wants: a blink is a form field's way of
            // asking for attention, not a page's.
            gpui_kit::base::Theme::global_mut(cx).cursor_blink = CursorBlink::steady();
            install_fonts(cx);

            let bounds = Bounds::centered(None, size(px(1240.), px(840.)), cx);
            open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("MeowDown".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                move |window, cx| {
                    cx.new(|cx| editor::MeowDown::with_path(target.clone(), window, cx))
                },
            )
            .expect("failed to open the MeowDown window");
        });
}
