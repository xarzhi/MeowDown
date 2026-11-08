//! The editor view: a Typora-style document surface built from typed blocks.
//!
//! Inactive blocks are rendered as rich Markdown through `TextView`; the block
//! holding the caret turns into a real text field, so editing stays live while
//! the rest of the page already looks like the finished document.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, Theme, ThemeColor, ThemeMode,
    button::{Button, ButtonVariants as _},
    input::{
        Editor, EditorState, Input as TextInput, InputEvent, InputState, MoveDown, MoveLeft, MoveRight,
        MoveUp, Paste, Textarea, TextareaState,
    },
    menu::{DropdownMenu as _, PopupMenuItem},
    scroll::{Scrollbar, ScrollbarMode},
    switch::Switch,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::base::input::{
    Editor as BaseEditor, FoldRange, HighlightStyleResolver, InputEdit, InputEditorStyle,
    InputHighlighter, Rope,
};

use crate::assoc;
use crate::inline;
use crate::math;
use crate::model::{self, BlockKind, PlainBlock};
use crate::settings::{self, ImageHost, ImageMode, Settings};
use crate::shellops;

/// Which page the settings dialog shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Images,
    Shortcuts,
}

/// What the left panel lists.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarTab {
    /// Every Markdown file sitting in the folder of the open document.
    Files,
    /// The headings of the open document.
    Outline,
}

/// How often the folder listing is allowed to hit the disk again. Cheap
/// enough to keep a plain `read_dir` in the render path, timed so that a file
/// created in Explorer shows up while the window is still open.
const FOLDER_RESCAN: Duration = Duration::from_millis(1500);

/// How long the editor waits after the last keystroke before it writes the
/// document back to disk when auto-save is on.
const AUTOSAVE_DELAY: Duration = Duration::from_millis(900);

/// Editable text fields inside the settings dialog.
struct SettingsFields {
    endpoint: Entity<TextareaState>,
    token: Entity<TextareaState>,
    field: Entity<TextareaState>,
    url_path: Entity<TextareaState>,
    assets_dir: Entity<TextareaState>,
}

/// One line of the shortcut cheat sheet.
struct Shortcut {
    keys: &'static str,
    action: &'static str,
}

/// The Typora shortcut set this editor implements, shown in settings.
const SHORTCUTS: &[(&str, &[Shortcut])] = &[
    (
        "文件",
        &[
            Shortcut { keys: "Ctrl+N", action: "新建文档" },
            Shortcut { keys: "Ctrl+O", action: "打开文档" },
            Shortcut { keys: "Ctrl+S", action: "保存" },
            Shortcut { keys: "Ctrl+Shift+S", action: "另存为" },
            Shortcut { keys: "Ctrl+,", action: "打开设置" },
            Shortcut { keys: "Ctrl+W", action: "关闭窗口" },
        ],
    ),
    (
        "段落",
        &[
            Shortcut { keys: "Ctrl+1 … 6", action: "一到六级标题" },
            Shortcut { keys: "Ctrl+0", action: "变回正文段落" },
            Shortcut { keys: "Ctrl+=", action: "标题级别升一级" },
            Shortcut { keys: "Ctrl+-", action: "标题级别降一级" },
            Shortcut { keys: "Ctrl+Shift+K", action: "代码块（也可 ``` + 回车）" },
            Shortcut { keys: "Ctrl+T", action: "插入表格" },
            Shortcut { keys: "Ctrl+Shift+M", action: "插入公式" },
            Shortcut { keys: "Ctrl+Shift+Q", action: "引用" },
            Shortcut { keys: "Ctrl+Shift+[", action: "有序列表" },
            Shortcut { keys: "Ctrl+Shift+]", action: "无序列表" },
            Shortcut { keys: "Ctrl+Shift+X", action: "任务列表" },
            Shortcut { keys: "Ctrl+Shift+I", action: "插入图片" },
        ],
    ),
    (
        "表格",
        &[
            Shortcut { keys: "Enter", action: "跳到下一个单元格，最后一个再按离开表格" },
            Shortcut { keys: "Ctrl+Enter", action: "在当前行下面插入一行" },
            Shortcut { keys: "Ctrl+Shift+Backspace", action: "删除当前行" },
        ],
    ),
    (
        "格式",
        &[
            Shortcut { keys: "Ctrl+B", action: "加粗" },
            Shortcut { keys: "Ctrl+I", action: "斜体" },
            Shortcut { keys: "Ctrl+U", action: "下划线" },
            Shortcut { keys: "Alt+Shift+5", action: "删除线" },
            Shortcut { keys: "Ctrl+Shift+`", action: "行内代码（只包住选中的文字）" },
            Shortcut { keys: "Ctrl+K", action: "超链接" },
            Shortcut { keys: "Ctrl+单击", action: "用浏览器打开链接" },
            Shortcut { keys: "Ctrl+\\", action: "清除格式" },
        ],
    ),
    (
        "视图与其他",
        &[
            Shortcut { keys: "Ctrl+/", action: "源码模式" },
            Shortcut { keys: "Ctrl+Shift+L", action: "显示/隐藏侧边栏" },
            Shortcut { keys: "双击图片", action: "全屏看图（滚轮缩放、拖动平移、← → 翻页）" },
            Shortcut { keys: "F8", action: "专注模式" },
            Shortcut { keys: "F11", action: "全屏" },
            Shortcut { keys: "Ctrl+L", action: "选中当前行" },
            Shortcut { keys: "Ctrl+Home / End", action: "跳到首段 / 末段" },
            Shortcut { keys: "Alt+↑ / ↓", action: "上下移动当前段落" },
            Shortcut { keys: "Esc", action: "退出当前段落编辑" },
        ],
    ),
];

/// Where the bundled demo document lives, shown on a cold start.
pub const SAMPLE: &str = include_str!("../assets/welcome.md");

fn ui_font() -> &'static str {
    if cfg!(target_os = "windows") {
        "Microsoft YaHei UI"
    } else if cfg!(target_os = "macos") {
        "PingFang SC"
    } else {
        "Noto Sans CJK SC"
    }
}

/// The family code is set in.
///
/// JetBrains Mono, the face IntelliJ IDEA sets its editor in, ships with the
/// application (see `install_fonts` in `main.rs`). Asking for it when it did
/// not load would be asking for a family no machine has — GPUI panics on the
/// first line it cannot shape — so the platform's own monospace is the
/// fallback rather than a second best guess.
fn mono_font() -> &'static str {
    if *crate::code_face_loaded() {
        "JetBrains Mono"
    } else if cfg!(target_os = "windows") {
        "Cascadia Mono"
    } else if cfg!(target_os = "macos") {
        "Menlo"
    } else {
        "DejaVu Sans Mono"
    }
}

/// Line height as a multiple of the font size. The rendered text and the text
/// field both use it, so focusing a block never moves the lines around.
const LINE_RATIO: f32 = 1.45;
/// Width of the side panel. The tab switch on top of it uses the same number,
/// so the panel and its header read as one column.
const SIDEBAR_WIDTH: f32 = 232.0;
/// Vertical padding every block carries, focused or not.
const BLOCK_PAD_Y: f32 = 3.0;
/// Horizontal padding every block carries, focused or not.
const BLOCK_PAD_X: f32 = 6.0;
/// Font size inside a code fence.
const CODE_TEXT_SIZE: f32 = 13.5;
/// Font size inside a table cell. The same number is used by the static cell
/// and by the field that opens on top of it, so clicking a cell never resizes
/// the table.
const TABLE_TEXT: f32 = 13.5;
/// First-frame guess for the frame around the rows: our padding, the editor's
/// own insets and the one-pixel border. Every frame after that measures the
/// real value off the laid-out editor instead of trusting this number.
/// Everything a code block's height adds around its text: our padding, the
/// editor's own insets, the border — plus a few pixels of slack. The slack
/// matters: with the text exactly as tall as the viewport, rounding can leave
/// the editor a pixel to scroll, and an editor that *can* scroll swallows the
/// wheel instead of letting the page move under the pointer.
const CODE_CHROME: f32 = 40.0;
/// Width rows lose before text starts: the editor's 8px on either side and its
/// 6px left / 10px right inner insets.
const CODE_TEXT_INSET: f32 = 30.0;
/// Font size a display formula is set at.
const MATH_SIZE: f32 = 17.5;
/// How far the editor keeps its text off the left edge of its box. The prose it
/// replaces starts flush, so the box is shifted by this much to line them up.
const TEXT_INSET: Pixels = px(6.0);

/// What a brand-new file says while it has no words in it. A page that is
/// nothing but white is indistinguishable from a page that failed to load, and
/// the caret alone is a thin line easy to miss on a large screen.
const EMPTY_PAGE_HINT: &str = "从这里开始写…";

/// How much of the content area the reading column takes. The remainder is the
/// air on either side, so the text never runs edge to edge — and because this
/// is a share rather than a fixed width, the column keeps growing with the
/// window instead of stopping at some measure.
const PAGE_WIDTH_RATIO: f32 = 0.95;
/// Below this the column stops shrinking and the margins give way instead — on
/// a small window the text matters more than the air around it.
const PAGE_MIN_WIDTH: f32 = 360.0;

/// The air the picture viewer keeps around a picture that has just been fitted
/// to the window, so the arrows have somewhere to live.
const VIEWER_PAD: f32 = 64.0;
/// How far the wheel can take a picture. Below a tenth everything is a speck;
/// past eight times, one pixel of the picture is a block of screen.
const VIEWER_MIN_ZOOM: f32 = 0.1;
const VIEWER_MAX_ZOOM: f32 = 8.0;
/// A press that travels more than this is a drag, not a click.
const VIEWER_CLICK_SLOP: f32 = 4.0;




struct Block {
    id: u64,
    kind: BlockKind,
    text: SharedString,
    /// The field that edits the block. A paragraph and a code fence both need
    /// per-run styling while typing, so both are driven by a code editor: a
    /// plain text field would drop every bold and every code chip the moment
    /// the caret landed in the line.
    state: Entity<EditorState>,
    /// The inline runs the paragraph editor paints with, as
    /// `(byte range, style)`. Shared with the editor's highlighter, because
    /// that is the only channel through which a field can colour its own text.
    styles: Rc<RefCell<Vec<(Range<usize>, HighlightStyle)>>>,
    /// The prose the field is currently showing: the Markdown with the markers
    /// of every run the caret is *not* inside hidden. Source offsets in here
    /// are what turn a caret position back into a byte of the document.
    shown: Option<inline::Inline>,
    /// Which run of the source `shown` has its markers on show, as that run's
    /// span in the Markdown. Rebuilding the prose is a write into the field, so
    /// it may only happen when this changes — a caret that stays inside one run
    /// must not make the field rewrite itself under the keystroke being typed.
    shown_open: Option<Range<usize>>,
    /// Code fences get a real code editor so they stay syntax highlighted.
    /// The little language field in the corner of a code block.
    lang: Option<Entity<TextareaState>>,
    /// An image's path and a display formula's source are plain strings with
    /// nothing to style, so they keep a plain field of their own.
    plain: Option<Entity<TextareaState>>,
    _subscriptions: Vec<Subscription>,
}

impl Block {
    fn is_code(&self) -> bool {
        self.kind.is_code()
    }
}

/// A selection in the making: where the press landed, in the bytes of the prose
/// the field is showing (not of the Markdown behind it — the two differ by every
/// marker the prose is hiding).
struct Drag {
    block: u64,
    /// The end that stays put.
    anchor: usize,
    /// The end the pointer drags, as last placed.
    head: usize,
}

/// The field a block holds its caret in. A paragraph and a fence edit through
/// the paragraph editor; an image and a display formula through a plain field.
/// Only `focus_block` needs to name the difference, so it is spelled out here
/// once instead of at every focus site.
#[derive(Clone)]
enum FocusTarget {
    Editor(Entity<EditorState>),
    Plain(Entity<TextareaState>),
}

impl FocusTarget {
    fn focus(&self, window: &mut Window, cx: &mut App) {
        match self {
            FocusTarget::Editor(state) => state.update(cx, |state, cx| state.focus(window, cx)),
            FocusTarget::Plain(state) => state.update(cx, |state, cx| state.focus(window, cx)),
        }
    }
}

impl From<Entity<EditorState>> for FocusTarget {
    fn from(state: Entity<EditorState>) -> Self {
        FocusTarget::Editor(state)
    }
}

impl From<Entity<TextareaState>> for FocusTarget {
    fn from(state: Entity<TextareaState>) -> Self {
        FocusTarget::Plain(state)
    }
}

pub struct MeowDown {
    path: Option<PathBuf>,
    /// Whether the page is the bundled sample document.
    ///
    /// The sample has no path of its own, so "no file, no folder" used to mean
    /// "this is the sample". Now that it is only ever shown on the first
    /// launch, a page with no path is just an unnamed document, and the two
    /// need telling apart: the line above the page says something different
    /// for each.
    sample: bool,
    blocks: Vec<Block>,
    active_id: Option<u64>,
    /// A selection being dragged out inside one paragraph, if there is one.
    ///
    /// Prose is text until it is clicked, and only then becomes a field — which
    /// means the field never sees the press that started the drag, and a
    /// press-and-drag over a paragraph that is not being edited would select
    /// nothing at all. So the gesture is driven from here instead: the press
    /// remembers where it landed, and every move that arrives with the button
    /// still down walks the other end of the selection to the pointer.
    drag: Option<Drag>,
    next_id: u64,
    dirty: bool,
    source_mode: bool,
    source: Entity<TextareaState>,
    scroll: ScrollHandle,
    hint: SharedString,
    settings: Settings,
    show_settings: bool,
    settings_tab: SettingsTab,
    fields: SettingsFields,
    sidebar_visible: bool,
    /// How much of the panel is on screen, 0 when it is folded away and 1 when
    /// it is fully out. Kept apart from `sidebar_visible` — that one is where
    /// the user asked it to go, this one is where it is on the way there, so a
    /// second toggle can pick the panel up mid-flight instead of jumping.
    sidebar_pos: f32,
    /// The fold in flight, while the panel is sliding shut or open.
    sidebar_anim: Option<Slide>,
    /// Which list the left panel is showing.
    sidebar_tab: SidebarTab,
    /// Markdown files found next to the open document.
    folder_files: Vec<PathBuf>,
    /// The folder `folder_files` was read from, so a different document
    /// forces a rescan instead of showing the previous folder.
    folder_dir: Option<PathBuf>,
    /// Whether the cached listing walked subfolders.
    folder_deep: bool,
    /// The folder read as a tree, so the side panel can show it level by level.
    tree: Option<TreeNode>,
    /// When the listing was last read from disk.
    folder_scan_at: Option<Instant>,
    /// Folder of the last document that had a path, so a brand new document
    /// keeps listing the folder the user was working in.
    last_dir: Option<PathBuf>,
    /// A folder explicitly opened as a workspace. When set, the file list is
    /// every Markdown file under it, at any depth.
    root: Option<PathBuf>,
    focus_mode: bool,
    /// Image blocks currently being uploaded, so the UI can show a spinner.
    uploading: Vec<u64>,
    /// Result of the last association attempt, shown in the settings dialog.
    association: SharedString,
    /// Window-space bounds of every block, captured while painting. They turn
    /// a click into a caret position instead of "jump to the start of the line".
    bounds: Rc<RefCell<HashMap<u64, Bounds<Pixels>>>>,
    /// The code block whose language field currently holds focus.
    lang_focus: Option<u64>,
    /// The language list open under that field, while it is up.
    lang_menu: Option<LangMenu>,
    /// Window-space bounds of each fence's language field, captured while
    /// painting. The list hangs off the field, so it has to know where the field
    /// ended up — the block's own rectangle is not it.
    lang_bounds: Rc<RefCell<HashMap<u64, Bounds<Pixels>>>>,
    /// Images already downloaded from the web, keyed by URL.
    remote_images: HashMap<String, Arc<Image>>,
    /// URLs whose download is in flight, so a repaint does not start a second.
    remote_pending: std::collections::HashSet<String>,
    /// URLs that could not be fetched, so they are not retried every frame.
    remote_failed: std::collections::HashSet<String>,
    /// Laid-out row count of each code block, cached against the text and
    /// the width it was measured at.
    code_rows: Rc<RefCell<HashMap<u64, CodeRows>>>,
    /// Paragraphs that have already asked for the extra frame it takes to
    /// measure where their links sit. Remembered so a paragraph that is scrolled
    /// out of sight — and therefore never painted, never measured — cannot ask
    /// for a frame every time it is rendered.
    link_asked: Rc<RefCell<std::collections::HashSet<u64>>>,
    /// Document-level undo history. A step holds the whole document as it
    /// looked before a change, so Ctrl+Z keeps working after the caret has
    /// wandered into another paragraph.
    undo_stack: Vec<Step>,
    redo_stack: Vec<Step>,
    /// The block and moment the last history step was opened at. Typing in the
    /// same paragraph within a moment folds into that step.
    coalesce: Option<(u64, Instant)>,
    /// When the pending auto-save was requested, if one is in flight.
    autosave_at: Option<Instant>,
    /// A glide to a heading that the frame loop is still running.
    scroll_anim: Option<ScrollAnim>,
    /// Where the side panel's thumb sits, in slots: 0 for files, 1 for the
    /// outline. Animated, so it carries the fraction while it travels.
    tab_pos: f32,
    /// The slide of that thumb, while one is in flight.
    tab_anim: Option<Slide>,
    /// Folders the tree shows open, keyed by path.
    expanded: std::collections::HashSet<PathBuf>,
    /// The table cell being typed into, if any. A table is edited in place:
    /// clicking a cell puts a field inside it, and every keystroke is written
    /// straight back into the Markdown the block holds.
    table_edit: Option<TableCellEdit>,
    /// A heading a link asked for, waiting for the page it points into to be
    /// laid out. The bounds a glide needs only exist once the new blocks have
    /// been painted, so the jump is finished on a later frame.
    pending_anchor: Option<String>,
    /// The right-click menu of the file tree, while it is open.
    menu: Option<FileMenu>,
    /// The modal a file-tree command opened, while it is up.
    prompt: Option<Prompt>,
    /// Whether the "save before closing?" question is on screen.
    close_prompt: bool,
    /// Whether the window's close hook has been installed. It is registered on
    /// the first frame, where the view finally has a handle to give it.
    close_hooked: bool,
    /// Where the keyboard goes while the close question is on screen. The page
    /// keeps its caret, but it does not keep the keys: a focused card can take
    /// Enter before the field turns it into a line break.
    close_focus: FocusHandle,
    /// The full-window picture viewer, while a picture is being looked at.
    viewer: Option<Viewer>,
    /// Intrinsic size of a picture, keyed by the source it was written as.
    image_sizes: HashMap<String, Size<f32>>,
    /// The block whose picture was pressed last, and when.
    ///
    /// A double click on a picture is awkward: the first press opens the block
    /// for editing, which lays the Markdown source out above the picture and
    /// pushes it down. The second press then lands on that source row instead
    /// of the picture, so the press is remembered here and the block can still
    /// tell that the gesture was aimed at its picture.
    picture_press: Option<(u64, Instant)>,
}

/// One cell of a table, open for editing.
struct TableCellEdit {
    block: u64,
    /// `None` is the header row.
    row: Option<usize>,
    column: usize,
    state: Entity<InputState>,
    _subscription: Subscription,
}

/// What a right-click in the file tree was aimed at, and where the menu opened.
#[derive(Clone)]
struct FileMenu {
    path: PathBuf,
    /// Whether the row was a folder. A file row answers with the commands that
    /// act on a document, a folder row with the ones that act on a place.
    is_dir: bool,
    /// The click, in window coordinates — the menu opens at the pointer.
    at: Point<Pixels>,
}

/// One language a fenced block can be written in.
#[derive(Clone)]
struct LangChoice {
    /// What goes into the fence — a token syntect resolves to a syntax.
    token: String,
    /// What the list shows: the syntax's own name, which is what a reader
    /// recognises ("Rust", not "rs").
    name: String,
}

/// The drop-down under a fence's language field: every language syntect knows,
/// filtered by what has been typed into that field.
struct LangMenu {
    block: u64,
    /// The rows on offer right now. Recomputed on every keystroke, so the list
    /// is the answer to what the field says and nothing else.
    rows: Vec<LangChoice>,
    /// The row the keyboard is on.
    active: usize,
    /// How far the list has been scrolled, in pixels.
    scroll: f32,
}

/// A picture being looked at full-window.
///
/// It is a viewer rather than a preview because it is a small one: the wheel
/// zooms, the pointer drags the picture around, and the arrows walk the other
/// pictures of the same document, in reading order, without going back to the
/// page in between.
struct Viewer {
    /// Every picture the document holds, in the order the page shows them.
    items: Vec<ViewerItem>,
    /// Which of them is on show.
    at: usize,
    /// 1.0 is the picture fitted to the window; the wheel multiplies it.
    zoom: f32,
    /// How far the picture has been dragged from the middle.
    offset: Point<Pixels>,
    /// The press that is currently moving the picture, if one is.
    drag: Option<ViewerDrag>,
}

/// One picture the viewer can show.
struct ViewerItem {
    /// The block it came from, so closing the viewer can hand the page back.
    block: u64,
    /// The source exactly as the Markdown wrote it — a path or a URL.
    src: String,
    /// The alt text, which makes a decent caption when there is one.
    alt: String,
}

/// A press that has not been released yet.
struct ViewerDrag {
    /// Where the press went down.
    from: Point<Pixels>,
    /// The offset the picture had at that moment.
    origin: Point<Pixels>,
    /// How far the pointer has travelled since, so a press that did not move
    /// can be told from a drag.
    moved: f32,
}

/// A small modal the file tree asks for: a name to type, a delete to confirm, a
/// search box, or the properties card.
struct Prompt {
    kind: PromptKind,
    /// The file or folder the prompt acts on. For `NewFile` / `NewFolder` this
    /// is the folder the new thing is being named in.
    path: PathBuf,
    title: SharedString,
    /// The one text field a prompt has, when it has one at all.
    field: Option<Entity<TextareaState>>,
    /// A line under the field: what went wrong last time, or how many results
    /// a search found.
    note: SharedString,
    /// Search results, and the query they were found for.
    hits: Vec<PathBuf>,
    hits_query: String,
    /// The rows of the properties card.
    rows: Vec<(String, String)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    Rename,
    Delete,
    Properties,
    Search,
}

/// One entry of the undo history: every block's text and type, plus where the
/// caret was.
#[derive(Clone, PartialEq, Eq)]
struct Step {
    blocks: Vec<(BlockKind, String)>,
    active: usize,
}

/// How long a burst of typing counts as one undo step.
const UNDO_GROUP: Duration = Duration::from_millis(700);
/// Steps kept before the oldest are dropped.
const UNDO_DEPTH: usize = 200;
/// How long the page takes to glide to a heading chosen in the outline.
const SCROLL_GLIDE: Duration = Duration::from_millis(260);
/// How long the side panel's thumb takes to travel between its two slots.
const TAB_GLIDE: Duration = Duration::from_millis(210);
/// How long the side panel itself takes to fold away, or to unfold.
const SIDEBAR_SLIDE: Duration = Duration::from_millis(190);

/// A scroll in flight, from one offset to another.
#[derive(Clone, Copy)]
struct ScrollAnim {
    from: Pixels,
    to: Pixels,
    start: Instant,
    duration: Duration,
}

impl ScrollAnim {
    /// Where the page should sit now, on an ease-out curve so it arrives softly
    /// instead of stopping dead.
    fn offset_at(&self, now: Instant) -> (Pixels, bool) {
        let elapsed = now.saturating_duration_since(self.start).as_secs_f32();
        let total = self.duration.as_secs_f32().max(0.001);
        let t = (elapsed / total).clamp(0., 1.);
        let eased = 1. - (1. - t).powi(3);
        (self.from + (self.to - self.from) * eased, t >= 1.)
    }
}

/// Which slot of the two-way switch a tab sits in, as a 0…1 fraction.
fn tab_slot(tab: SidebarTab) -> f32 {
    match tab {
        SidebarTab::Files => 0.,
        SidebarTab::Outline => 1.,
    }
}

/// A value gliding between two points — the side panel's thumb travelling to
/// its other slot, the panel itself folding away.
#[derive(Clone, Copy)]
struct Slide {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
}

impl Slide {
    fn value_at(&self, now: Instant) -> (f32, bool) {
        let elapsed = now.saturating_duration_since(self.start).as_secs_f32();
        let total = self.duration.as_secs_f32().max(0.001);
        let t = (elapsed / total).clamp(0., 1.);
        let eased = 1. - (1. - t).powi(3);
        (self.from + (self.to - self.from) * eased, t >= 1.)
    }
}

/// A code block's measured row count, keyed by the inputs that produced it.
#[derive(Clone, Copy)]
struct CodeRows {
    text_hash: u64,
    width: f32,
    rows: usize,
}

impl MeowDown {
    /// Open the editor on `path` when one was passed on the command line; on
    /// the bundled sample the very first time the program runs; otherwise on
    /// an empty page.
    pub fn with_path(
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = cx.new(|cx| {
            TextareaState::new(window, cx)
                .soft_wrap(true)
                .placeholder("Markdown 源码")
        });

        let field = |placeholder: &'static str,
                     window: &mut Window,
                     cx: &mut Context<Self>|
         -> Entity<TextareaState> {
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(1, 4)
                    .submit_on_enter(false)
                    .placeholder(placeholder)
            })
        };
        let fields = SettingsFields {
            endpoint: field("https://example.com/upload", window, cx),
            token: field("API Token", window, cx),
            field: field("file", window, cx),
            url_path: field("data.url", window, cx),
            assets_dir: field("assets", window, cx),
        };

        let settings = Settings::load();
        let mut this = Self {
            path: None,
            sample: false,
            blocks: Vec::new(),
            active_id: None,
            drag: None,
            next_id: 1,
            dirty: false,
            source_mode: false,
            source,
            scroll: ScrollHandle::new(),
            hint: SharedString::default(),
            settings,
            show_settings: false,
            settings_tab: SettingsTab::General,
            fields,
            sidebar_visible: true,
            sidebar_pos: 1.,
            sidebar_anim: None,
            sidebar_tab: SidebarTab::Outline,
            folder_files: Vec::new(),
            folder_dir: None,
            folder_deep: false,
            tree: None,
            folder_scan_at: None,
            last_dir: None,
            root: None,
            focus_mode: false,
            uploading: Vec::new(),
            association: SharedString::default(),
            bounds: Rc::new(RefCell::new(HashMap::new())),
            lang_focus: None,
            lang_menu: None,
            lang_bounds: Rc::new(RefCell::new(HashMap::new())),
            remote_images: HashMap::new(),
            remote_pending: std::collections::HashSet::new(),
            remote_failed: std::collections::HashSet::new(),
            code_rows: Rc::new(RefCell::new(HashMap::new())),
            link_asked: Rc::new(RefCell::new(std::collections::HashSet::new())),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            coalesce: None,
            autosave_at: None,
            scroll_anim: None,
            // Matches the default tab above: the thumb starts over "大纲", not
            // over "文件", so the first paint is already truthful.
            tab_pos: 1.,
            tab_anim: None,
            expanded: std::collections::HashSet::new(),
            table_edit: None,
            pending_anchor: None,
            menu: None,
            prompt: None,
            close_prompt: false,
            close_hooked: false,
            close_focus: cx.focus_handle(),
            viewer: None,
            image_sizes: HashMap::new(),
            picture_press: None,
        };

        // A folder on the command line opens that folder: the side panel lists
        // every Markdown file inside it. No note is opened along with it.
        let folder = path.as_ref().filter(|path| path.is_dir()).cloned();
        let path = path.filter(|path| path.is_file());
        if let Some(dir) = folder {
            this.adopt_root(dir);
        }
        // A note opened on its own brings its folder with it, the way a folder
        // does: the notes beside it are the thing a reader is most likely to
        // reach for next, and a trip through 打开文件夹 to get at them would be
        // a step the command line already knew the answer to.
        if let Some(current) = path.as_ref() {
            this.adopt_parent(current);
        }
        // Opening a folder opens the folder, not one of the notes inside it:
        // which note to read is the user's call, so the page starts blank and
        // waits for a click in the list.
        let folder_open = this.root.is_some();
        if let Some(current) = path.clone() {
            this.reveal_in_tree(&current);
        }

        // The bundled sample is a greeting, not a landing page: it is handed
        // out on the very first launch and never again, so a reader who opens
        // the program to write something lands on an empty page instead of a
        // file they never asked for. A launch that already has a document, or
        // a folder, spends nothing — the greeting is still there for the first
        // bare launch.
        let show_sample = path.is_none() && !folder_open && settings::claim_welcome();

        let initial = if show_sample {
            SAMPLE.to_string()
        } else {
            path.as_ref()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .unwrap_or_default()
        };
        let mut blocks = Vec::new();
        for plain in model::parse(&initial) {
            blocks.push(this.make_block(window, cx, plain));
        }
        if blocks.is_empty() {
            blocks.push(this.make_block(window, cx, PlainBlock::new(BlockKind::Paragraph, "")));
        }
        this.blocks = blocks;
        this.path = path;
        this.sample = show_sample;
        this.focus_first(window, cx);
        if let Some(title) = this.window_title() {
            window.set_window_title(&title);
        }
        this
    }

    // ---------------------------------------------------------------- blocks

    fn make_block(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        plain: PlainBlock,
    ) -> Block {
        let id = self.next_id;
        self.next_id += 1;
        let kind = plain.kind.clone();
        let text: SharedString = plain.text.clone().into();
        let (state, lang, plain, styles, _subscriptions) =
            self.build_state(id, kind.clone(), plain.text, window, cx);
        Block {
            id,
            kind,
            text,
            state,
            styles,
            shown: None,
            shown_open: None,
            lang,
            plain,
            _subscriptions,
        }
    }

    /// Create the fields that back a block: the text field every block edits,
    /// plus the little language field a fence carries in its corner.
    fn build_state(
        &mut self,
        id: u64,
        kind: BlockKind,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (
        Entity<EditorState>,
        Option<Entity<TextareaState>>,
        Option<Entity<TextareaState>>,
        Rc<RefCell<Vec<(Range<usize>, HighlightStyle)>>>,
        Vec<Subscription>,
    ) {
        let is_code = kind.is_code();
        let plain_needed = kind.is_image() || kind.is_math();
        let mut subscriptions = Vec::new();
        let styles: Rc<RefCell<Vec<(Range<usize>, HighlightStyle)>>> =
            Rc::new(RefCell::new(Vec::new()));

        // A paragraph's field holds the prose, not the Markdown: markers are
        // shown only for the run the caret is inside, which is decided later.
        let seed = if kind.is_code() {
            text.clone()
        } else {
            inline::render(&text).text
        };
        let runs = styles.clone();
        let fence_language = match &kind {
            BlockKind::Code(language) => language.clone(),
            _ => None,
        };
        let state = cx.new(|cx| {
            let editor = EditorState::new(window, cx)
                .line_number(false)
                .folding(false)
                // Markdown punctuation is typed here, not code: a `*` must stay
                // one character instead of closing a pair by itself.
                .auto_close(false)
                .smart_indent(false)
                .indent_guides(false)
                // A code editor keeps half a viewport of empty rows below the
                // last line so the caret can sit at the top of the screen. In
                // a document that is slack to scroll through, so it is pinned
                // to zero: the text is exactly as tall as the box around it.
                .scroll_beyond_last_line(Some(0))
                .scroll_bounce(false);
            let editor = if seed.is_empty() {
                editor
            } else {
                editor.default_value(seed.clone())
            };
            match &fence_language {
                Some(language) => editor.language(language.clone()),
                None => editor,
            }
        });
        state.update(cx, |state, cx| {
            if !is_code {
                let runs = runs.clone();
                state.set_highlighter_factory(
                    Rc::new(move |_| {
                        Some(Box::new(InlineHighlighter {
                            runs: runs.clone(),
                        }) as Box<dyn gpui_kit::base::input::InputHighlighter>)
                    }),
                    cx,
                );
            }
        });
        subscriptions.push(cx.subscribe_in(
            &state,
            window,
            move |this: &mut MeowDown,
                  _: &Entity<EditorState>,
                  event: &InputEvent,
                  window: &mut Window,
                  cx: &mut Context<MeowDown>| this.on_block_event(id, event, window, cx),
        ));
        // Moving the caret is not an input event, so a run would only open or
        // close on the next keystroke. Watching the field is what makes the
        // markers follow the caret as it travels — but the redraw is all this
        // does: rebuilding the prose is a write into the field, and doing it
        // here meant overwriting a keystroke that had not reached the document
        // yet. `render_block` decides, and it knows better.
        subscriptions.push(cx.observe_in(&state, window, move |_, _, _, cx| {
            cx.notify();
        }));

        // Neither a fence nor a paragraph ever scrolls inside itself: the box
        // around the text is sized to fit, so a scrollbar would only ever be a
        // lie — and a field that *can* scroll takes the wheel away from the
        // page.
        state.update(cx, |state, _| {
            state.set_scrollbar_renderer(Some(Rc::new(|_, _, _| {
                div().into_any_element()
            })));
        });
        let _ = &fence_language;

        // An image's path and a display formula's source are plain strings
        // with nothing to style, so they keep a plain field.
        let plain = if plain_needed {
            let seed = text.clone();
            let submit = !kind.is_math();
            let input = cx.new(|cx| {
                let state = TextareaState::new(window, cx)
                    .soft_wrap(true)
                    .auto_grow(1, 60)
                    .submit_on_enter(submit);
                if seed.is_empty() {
                    state
                } else {
                    state.default_value(seed.clone())
                }
            });
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this: &mut MeowDown,
                      _: &Entity<TextareaState>,
                      event: &InputEvent,
                      window: &mut Window,
                      cx: &mut Context<MeowDown>| this.on_block_event(id, event, window, cx),
            ));
            Some(input)
        } else {
            None
        };

        let lang = if is_code {
            let initial = match &kind {
                BlockKind::Code(Some(language)) => language.clone(),
                _ => String::new(),
            };
            let input = cx.new(|cx| {
                let state = TextareaState::new(window, cx)
                    .auto_grow(1, 1)
                    .submit_on_enter(false)
                    .placeholder("语言");
                if initial.is_empty() {
                    state
                } else {
                    state.default_value(initial.clone())
                }
            });
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this: &mut MeowDown,
                      _: &Entity<TextareaState>,
                      event: &InputEvent,
                      window: &mut Window,
                      cx: &mut Context<MeowDown>| this.on_lang_event(id, event, window, cx),
            ));
            Some(input)
        } else {
            None
        };

        (state, lang, plain, styles, subscriptions)
    }

    /// The text of a block, read from the field that owns it.
    fn block_text(&self, index: usize, cx: &App) -> String {
        let block = &self.blocks[index];
        match &block.plain {
            Some(plain) => plain.read(cx).value().to_string(),
            None => block.state.read(cx).value().to_string(),
        }
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.blocks.iter().position(|block| block.id == id)
    }

    /// Replace a block's Markdown, in the model *and* in the field that owns
    /// it.
    ///
    /// The two have to move together. A block with a plain field — an image's
    /// path, a formula's source — keeps its canonical text in that field:
    /// `block_text` reads it back on every change event, so writing only the
    /// model left the field holding the old string, and the next change event
    /// (which the write itself caused) took that old string and put it back
    /// over the new one. That is why an uploaded image kept showing its local
    /// path although the upload had returned a URL.
    fn set_block_text(
        &mut self,
        index: usize,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.blocks[index].text = text.into();
        self.blocks[index].shown = None;
        self.blocks[index].shown_open = None;
        if let Some(plain) = self.blocks[index].plain.clone() {
            plain.update(cx, |plain, cx| {
                plain.set_value(text.to_string(), window, cx)
            });
        } else {
            let prose = inline::render(text).text;
            let state = self.blocks[index].state.clone();
            state.update(cx, |state, cx| state.set_value(prose, window, cx));
        }
        self.dirty = true;
    }

    fn active_index(&self) -> Option<usize> {
        self.active_id.and_then(|id| self.index_of(id))
    }

    fn focus_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_block(0, window, cx);
    }

    fn focus_block(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(block) = self.blocks.get(index) else {
            return;
        };
        let id = block.id;
        self.active_id = Some(id);
        let field: FocusTarget = match &block.plain {
            Some(plain) => plain.clone().into(),
            None => block.state.clone().into(),
        };
        field.focus(window, cx);
        // Asking once is not enough. Becoming active swaps this block's rich
        // text for a field, and by the time the next frame is drawn the focus
        // is back on the block that had it before — the blur of that block
        // arrives after this update. The field therefore takes the focus again
        // at the end of the cycle, once that has settled. Without this the
        // caret never appears and the keyboard goes nowhere.
        let again = field.clone();
        cx.defer_in(window, move |this, window, cx| {
            if this.active_id == Some(id) {
                again.focus(window, cx);
            }
        });
        cx.notify();
    }

    fn activate(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        if self.blocks[index].kind.is_rule() {
            return;
        }
        self.source_mode = false;
        self.focus_block(index, window, cx);
        // Clicking a heading in the outline has to bring it into view: focusing
        // a field does not scroll the page on its own.
        self.scroll_to_block(id);
        cx.notify();
    }

    /// Scroll so a block sits just under the top edge of the reading area.
    ///
    /// The page glides there instead of teleporting: a long document jumping in
    /// one frame loses the reader's place, so the offset is animated over a
    /// fraction of a second and driven by the frame loop.
    fn scroll_to_block(&mut self, id: u64) {
        let Some(bounds) = self.bounds.borrow().get(&id).copied() else {
            return;
        };
        let viewport = self.scroll.bounds();
        if viewport.size.height <= px(1.) {
            return;
        }
        let offset = self.scroll.offset();
        let delta = bounds.top() - viewport.top() - px(24.);
        let y = (offset.y - delta)
            .min(px(0.))
            .max(-self.scroll.max_offset().y);
        if (y - offset.y).abs() < px(1.) {
            return;
        }
        self.scroll_anim = Some(ScrollAnim {
            from: offset.y,
            to: y,
            start: Instant::now(),
            duration: SCROLL_GLIDE,
        });
    }

    /// Put the caret where the pointer landed instead of at the start of the
    /// paragraph. The block records its bounds while painting, so the click can
    /// be resolved to a line and a column.
    ///
    /// The prose on screen is not the Markdown source: `**`, `` ` `` and the
    /// `[…](…)` brackets are gone, and a formula is a run of blanks the width
    /// of the typeset box. So the point is measured against the *rendered*
    /// text — the same string, font and width the paragraph was laid out with —
    /// and then walked back through the scanner's offset map to the byte the
    /// caret really belongs at. Measuring the source instead is what made the
    /// caret land somewhere else entirely.
    fn activate_at(
        &mut self,
        id: u64,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        if self.blocks[index].kind.is_rule() {
            return;
        }
        let bounds = self.bounds.borrow().get(&id).copied();
        if self.blocks[index].is_code() {
            self.focus_block(index, window, cx);
            return;
        }
        // A table brings its own cell editors; focusing the paragraph field
        // behind it would put a caret in the Markdown the grid is drawn from,
        // where typing would edit text nobody can see.
        if self.blocks[index].kind.is_table() {
            self.active_id = Some(id);
            cx.notify();
            return;
        }
        let source = self.blocks[index].text.to_string();
        let kind = self.blocks[index].kind.clone();
        // Which shape the paragraph is in right now: at rest it shows the
        // prose, with the markers of every run hidden; while it is being
        // edited it shows that same prose plus the markers of the one run the
        // caret sits in. The pointer has to be measured against the one that
        // is actually on screen, or it points at the wrong character.
        let on_screen = (self.active_id == Some(id))
            .then(|| self.blocks[index].shown.clone())
            .flatten();
        self.source_mode = false;
        self.focus_block(index, window, cx);

        let Some(bounds) = bounds else {
            return;
        };
        let (size, bold, color) = block_type_style(&kind, &cx.theme().colors);
        let x = (position.x - bounds.left()).max(px(0.0));
        let y = (position.y - bounds.top()).max(px(0.0));
        let width = bounds.size.width;
        if width <= px(16.) {
            return;
        }

        // The pointer is measured against what is on screen and walked back to
        // the byte of the Markdown it came from. Measuring the Markdown itself
        // would put the caret several characters off, because every `**` that
        // comes back into view pushes the text along.
        let inline = on_screen.unwrap_or_else(|| inline::render(&source));
        let (display, _, _, gaps) = expand_inline_math(&inline, size, color, window);
        let chips = code_chips(&inline.spans);
        let (hit, _) = caret_at_point(&display, size, bold, &chips, width, x, y, window);
        let caret = display_to_source(&inline, &gaps, hit);
        self.place_caret(index, caret, window, cx);
    }

    /// Put the caret at `caret` — a byte offset into the block's Markdown — and
    /// give the field the shape that caret asks for: the run it lands in opens
    /// its markers. Used both by a click, which has a pointer to translate, and
    /// by a click on the page around the text, which has no pointer inside a
    /// line and simply wants the caret at one end of the nearest block.
    fn place_caret(&mut self, index: usize, caret: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(block) = self.blocks.get(index) else {
            return;
        };
        let id = block.id;
        let source = block.text.to_string();
        // `set_selected_range` speaks UTF-8 byte offsets, like the parser does.
        let caret = boundary(&source, caret.min(source.len()));
        // A click on the very edge of an open run belongs *inside* it: the
        // markers are only a character or two wide, and landing one pixel
        // short of them used to close the run the click was aiming at, which
        // made `**`, `` ` `` and the address of a link impossible to put a
        // caret into at all.
        let caret = self.snap_into_open_run(index, &source, caret);

        // Put the field into the shape the caret asks for — the run under the
        // pointer opens up — and place the caret in that shape.
        let (view, placed) = view_at(&source, caret);
        let caret = view.source_point(placed);
        let value = view.text.clone();
        let state = self.blocks[index].state.clone();
        state.update(cx, |state, cx| {
            state.set_value(value, window, cx);
            state.set_selected_range(placed..placed, cx);
        });
        self.blocks[index].shown_open = inline::render(&source)
            .element_at(caret)
            .map(|element| element.span.clone());
        self.blocks[index].shown = Some(view);
        // The press that opened the paragraph also opens a drag. The button may
        // still be held, and the field did not exist when it went down, so this
        // is the only moment the anchor can be recorded.
        self.drag = Some(Drag {
            block: id,
            anchor: placed,
            head: placed,
        });
        cx.notify();
    }

    /// Whether a press fell inside the box of a block that is on the page.
    ///
    /// Every block records where it ended up: prose through the probe in
    /// `render_block`, and the blocks that bring their own interior — a fence,
    /// a picture, a formula, a table — through `block_probe`. Only ids the
    /// document still has count, so a rectangle left behind by a block that has
    /// since been deleted cannot swallow a click.
    fn press_inside_block(&self, at: Point<Pixels>) -> bool {
        let bounds = self.bounds.borrow();
        self.blocks.iter().any(|block| {
            bounds.get(&block.id).is_some_and(|area| {
                at.x >= area.left()
                    && at.x <= area.right()
                    && at.y >= area.top()
                    && at.y <= area.bottom()
            })
        })
    }

    /// An invisible layer that records where a block sits.
    ///
    /// A block's own box is not painted by anything that reports its bounds, so
    /// the same trick the prose uses — an absolutely positioned canvas that
    /// takes no room in the layout — is hung inside the block to note down the
    /// rectangle a press can be measured against.
    fn block_probe(&self, id: u64) -> AnyElement {
        let map = self.bounds.clone();
        canvas(
            move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
                map.borrow_mut().insert(id, bounds);
            },
            |_: Bounds<Pixels>, _: (), _: &mut Window, _: &mut App| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }

    /// Record where a fence's language field landed, so the language list can
    /// hang off it. The block's own rectangle will not do: the field is a small
    /// box in its bottom-right corner, and the list has to open from there.
    fn lang_probe(&self, id: u64) -> AnyElement {
        let map = self.lang_bounds.clone();
        canvas(
            move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
                map.borrow_mut().insert(id, bounds);
            },
            |_: Bounds<Pixels>, _: (), _: &mut Window, _: &mut App| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }

    /// A click on the page but not on a line of text — the padding around the
    /// column, the space below the last paragraph. Focusing that empty space is
    /// what every editor does: the caret goes to the nearest block, at the end
    /// when the pointer is past it and at the start when it is above.
    fn focus_page_at(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.source_mode || self.blocks.is_empty() {
            return;
        }
        // A press that landed on a block belongs to that block. Images and
        // formulas turn their source on from their own handler, a fence hands
        // the press to its editor, a table opens a cell — and the page handler
        // runs *after* them, because it sits on the container. Without this
        // check the page resolved the same press to the nearest line of prose
        // and took the focus back, so a click on an image flipped its source on
        // and off again in the same frame.
        if self.press_inside_block(position) {
            return;
        }
        let bounds = self.bounds.borrow().clone();
        let mut best: Option<(usize, Pixels)> = None;
        for (index, block) in self.blocks.iter().enumerate() {
            let Some(area) = bounds.get(&block.id) else {
                continue;
            };
            let gap = if position.y < area.top() {
                area.top() - position.y
            } else if position.y > area.bottom() {
                position.y - area.bottom()
            } else {
                px(0.)
            };
            if best.is_none_or(|(_, closest)| gap < closest) {
                best = Some((index, gap));
            }
        }
        let Some((index, _)) = best else {
            return;
        };
        let kind = self.blocks[index].kind.clone();
        // Blocks that bring their own editor — a fence, a table cell, a formula
        // — are entered by their own click handlers; the page click only hands
        // them the focus.
        if kind.is_rule() || kind.is_image() || kind.is_code() || kind.is_table() || kind.is_math() {
            return;
        }
        let below = bounds
            .get(&self.blocks[index].id)
            .is_none_or(|area| position.y >= area.top() + area.size.height / 2.);
        self.focus_block(index, window, cx);
        let end = self.blocks[index].text.len();
        self.place_caret(index, if below { end } else { 0 }, window, cx);
    }

    /// Nudge a click that landed on the very edge of the run that is open back
    /// inside it.
    ///
    /// A marker is one or two characters wide, so half of its pixels resolve to
    /// the position *before* it and half to the position after. Only the
    /// positions strictly inside a run keep its markers on show; the edges fall
    /// out of it, the prose is rebuilt without them, and the character the
    /// pointer was aiming at disappears under it. A caret asked for at either
    /// edge is therefore moved one byte in, which is still the marker itself —
    /// the thing the click was after — and the run stays open.
    fn snap_into_open_run(&self, index: usize, source: &str, caret: usize) -> usize {
        let Some(span) = self.blocks[index].shown_open.clone() else {
            return caret;
        };
        if span.start >= span.end || span.end > source.len() {
            return caret;
        }
        if caret == span.start {
            return span.start + 1;
        }
        if caret == span.end {
            return span.end.saturating_sub(1).max(span.start);
        }
        caret
    }

    /// Walk the far end of the drag to the pointer.
    ///
    /// The field itself cannot do this: it never saw the press, so it has no
    /// anchor to drag from. Everything here is measured against what the field
    /// is *showing* — the prose with its markers hidden — because that is what
    /// the pointer is pointing at and what the selection has to speak in.
    fn drag_over(
        &mut self,
        id: u64,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(anchor) = self
            .drag
            .as_ref()
            .filter(|drag| drag.block == id)
            .map(|drag| drag.anchor)
        else {
            return;
        };
        let Some(index) = self.index_of(id) else {
            return;
        };
        if self.blocks[index].is_code() || self.blocks[index].kind.is_table() {
            return;
        }
        let Some(bounds) = self.bounds.borrow().get(&id).copied() else {
            return;
        };
        let Some(shown) = self.blocks[index].shown.clone() else {
            return;
        };
        let (size, bold, _) = block_type_style(&self.blocks[index].kind, &cx.theme().colors);
        let x = (position.x - bounds.left()).max(px(0.0));
        let y = (position.y - bounds.top()).max(px(0.0));
        let (hit, _) = caret_at_point(
            &shown.text,
            size,
            bold,
            &code_chips(&shown.spans),
            bounds.size.width,
            x,
            y,
            window,
        );
        let head = boundary(&shown.text, hit.min(shown.text.len()));
        let anchor = anchor.min(shown.text.len());
        let state = self.blocks[index].state.clone();
        // The range has to be handed over the right way round — a `start`
        // past its `end` is clamped to a bare caret — while `reversed` keeps
        // the direction: dragging left of the anchor has to leave the caret on
        // the left, or typing after the drag would type at the wrong end.
        let (from, to) = if head < anchor {
            (head, anchor)
        } else {
            (anchor, head)
        };
        state.update(cx, |state, cx| {
            state.set_selected_range_with_direction(from..to, head < anchor, cx);
        });
        self.drag = Some(Drag {
            block: id,
            anchor,
            head,
        });
        cx.notify();
    }

    /// Finish a drag.
    ///
    /// The field never saw the press, so its own mouse-up handling believes
    /// nothing is being dragged: it drops the caret where the pointer is, which
    /// folds away the selection the drag just made. Putting the range back is
    /// this method's whole job.
    fn end_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if drag.anchor == drag.head {
            return;
        }
        let Some(index) = self.index_of(drag.block) else {
            return;
        };
        if self.blocks[index].is_code() || self.blocks[index].kind.is_table() {
            return;
        }
        let state = self.blocks[index].state.clone();
        let (from, to) = if drag.head < drag.anchor {
            (drag.head, drag.anchor)
        } else {
            (drag.anchor, drag.head)
        };
        let current = state.read(cx).selected_range();
        if current == (from..to) {
            return;
        }
        state.update(cx, |state, cx| {
            state.set_selected_range_with_direction(from..to, drag.head < drag.anchor, cx);
        });
        cx.notify();
    }

    fn on_block_event(
        &mut self,
        id: u64,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        match event {
            InputEvent::Change => {
                if !self.blocks[index].is_code() && self.blocks[index].plain.is_none() {
                    self.apply_paragraph_edit(index, window, cx);
                    return;
                }
                let text = self.block_text(index, cx);
                if self.blocks[index].text.as_ref() != text {
                    // The model still holds the pre-keystroke text, so this is
                    // the moment to snapshot it for Ctrl+Z.
                    self.remember(id);
                    self.blocks[index].text = text.into();
                    self.dirty = true;
                    self.apply_shortcut(index, window, cx);
                    self.schedule_autosave(window, cx);
                    cx.notify();
                }
            }
            InputEvent::PressEnter { shift, .. } => {
                // A paragraph never gets here: its Enter is taken in the
                // capture phase, before the field can insert a line break. What
                // is left are the single-line fields — an image's path, a
                // formula's source — where Enter means "move on".
                if !shift && self.blocks[index].plain.is_some() {
                    self.remember(id);
                    self.split_block(index, window, cx);
                    self.schedule_autosave(window, cx);
                }
            }
            InputEvent::Focus => {
                self.active_id = Some(id);
                cx.notify();
            }
            InputEvent::Blur => {
                if self.active_id == Some(id) && self.lang_focus != Some(id) {
                    self.active_id = None;
                    // Nothing in this block holds the keyboard any more, so the
                    // language list it opened goes with the focus.
                    self.lang_menu = None;
                    // The press that started a drag can be released outside the
                    // block, where its mouse-up is never seen; losing the focus
                    // is the other moment the gesture is definitely over.
                    self.drag = None;
                    // The prose on screen goes back to hiding every marker, so
                    // the view kept here would be a picture of a shape that is
                    // no longer drawn — and the next click would be measured
                    // against it.
                    self.blocks[index].shown = None;
                    self.blocks[index].shown_open = None;
                    cx.notify();
                }
            }
        }
    }

    /// What a fence's language field currently says.
    fn lang_query(&self, block: u64, cx: &App) -> String {
        self.index_of(block)
            .and_then(|index| self.blocks[index].lang.clone())
            .map(|field| field.read(cx).value().trim().to_string())
            .unwrap_or_default()
    }

    /// Open the list under a fence's language field.
    ///
    /// Opening is the whole point of focusing that field: the set of languages
    /// a fence can be written in is not something a reader is expected to
    /// remember — Typora shows it, and so does this.
    fn open_lang_menu(&mut self, block: u64, cx: &mut Context<Self>) {
        let rows = lang_matches(&self.lang_query(block, cx));
        self.lang_menu = Some(LangMenu {
            block,
            rows,
            active: 0,
            scroll: 0.,
        });
        cx.notify();
    }

    /// Re-read the field and rebuild the rows.
    ///
    /// Every keystroke lands here, which is what makes the panel a completion
    /// rather than a picker: the list narrows as the name is typed, and a row
    /// that starts with what was typed comes before one that merely mentions it.
    fn refresh_lang_menu(&mut self, cx: &mut Context<Self>) {
        let Some(block) = self.lang_menu.as_ref().map(|menu| menu.block) else {
            return;
        };
        let rows = lang_matches(&self.lang_query(block, cx));
        if let Some(menu) = self.lang_menu.as_mut() {
            menu.rows = rows;
            menu.active = 0;
            menu.scroll = 0.;
        }
        cx.notify();
    }

    fn close_lang_menu(&mut self, cx: &mut Context<Self>) {
        if self.lang_menu.take().is_some() {
            cx.notify();
        }
    }

    /// Move the keyboard's row, scrolling the list to keep it in sight.
    ///
    /// The ends wrap, so holding a key walks the whole list instead of stopping
    /// dead at the last row.
    fn lang_menu_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(menu) = self.lang_menu.as_mut() else {
            return;
        };
        if menu.rows.is_empty() {
            return;
        }
        let count = menu.rows.len() as isize;
        menu.active = (menu.active as isize + delta).rem_euclid(count) as usize;
        let top = menu.active as f32 * LANG_ROW;
        let view = LANG_ROWS_SHOWN as f32 * LANG_ROW;
        if top < menu.scroll {
            menu.scroll = top;
        } else if top + LANG_ROW > menu.scroll + view {
            menu.scroll = top + LANG_ROW - view;
        }
        cx.notify();
    }

    /// Put the row the keyboard is on into the field.
    fn accept_lang_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.lang_menu.as_ref() else {
            return;
        };
        let Some(choice) = menu.rows.get(menu.active).cloned() else {
            return;
        };
        self.choose_lang(choice, window, cx);
    }

    /// Write a language into the fence.
    ///
    /// The field is written first, so what the reader sees is the truth; the
    /// block follows it through the same path typing would take.
    fn choose_lang(&mut self, choice: LangChoice, window: &mut Window, cx: &mut Context<Self>) {
        let Some(block) = self.lang_menu.as_ref().map(|menu| menu.block) else {
            return;
        };
        let Some(index) = self.index_of(block) else {
            return;
        };
        self.remember(block);
        if let Some(field) = self.blocks[index].lang.clone() {
            field.update(cx, |state, cx| state.set_value(choice.token.clone(), window, cx));
            // The press that chose a row took the focus off the box; a language
            // is often half remembered, and clicking back in to fix a letter
            // would be a silly little tax.
            field.update(cx, |state, cx| state.focus(window, cx));
        }
        self.set_fence_language(index, Some(choice.token), cx);
        // Writing the field fires a Change, which would rebuild the list; the
        // choice is in, so the list has done its job.
        self.lang_menu = None;
        cx.notify();
    }

    /// Point a fence at another language: the block, its highlighter and the
    /// Markdown all move together.
    fn set_fence_language(
        &mut self,
        index: usize,
        language: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.blocks[index].kind = BlockKind::Code(language.clone());
        if let Some(language) = language {
            let state = self.blocks[index].state.clone();
            state.update(cx, |state, cx| state.set_highlighter(language, cx));
        }
        self.dirty = true;
        cx.notify();
    }

    /// The language field in the corner of a code block.
    fn on_lang_event(
        &mut self,
        id: u64,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        match event {
            InputEvent::Change => {
                let Some(lang) = self.blocks[index].lang.clone() else {
                    return;
                };
                let value = lang.read(cx).value().trim().to_string();
                self.remember(id);
                let language = if value.is_empty() { None } else { Some(value) };
                self.set_fence_language(index, language, cx);
                // Typing narrows the list; the first keystroke after the escape
                // key opens it again.
                if self.lang_menu.is_some() {
                    self.refresh_lang_menu(cx);
                } else {
                    self.open_lang_menu(id, cx);
                }
            }
            InputEvent::Focus => {
                self.lang_focus = Some(id);
                self.active_id = Some(id);
                self.open_lang_menu(id, cx);
            }
            InputEvent::Blur => {
                if self.lang_focus == Some(id) {
                    self.lang_focus = None;
                }
                // The list stays up across the blur on purpose: pressing a row
                // means leaving the field first, and closing here would take
                // the row away before the press could land on it. A press
                // anywhere else goes to the panel's backdrop, which closes it.
                cx.notify();
            }
            _ => {}
        }
    }


    /// Write what was typed in a paragraph back into its Markdown.
    ///
    /// The field holds the *prose*, so its text is not the document: a run the
    /// caret is inside shows its markers, and every other run does not. What
    /// changed is a replacement in that prose, and it is translated here into a
    /// replacement in the Markdown — the characters the field never showed keep
    /// their place in the source.
    fn apply_paragraph_edit(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.blocks[index].state.clone();
        let edited = state.read(cx).value().to_string();
        let Some(shown) = self.blocks[index].shown.clone() else {
            return;
        };
        if edited == shown.text || self.blocks[index].text.is_empty() && edited.is_empty() {
            return;
        }
        let (at, removed, inserted) = text_diff(&shown.text, &edited);
        let start = shown.source_at(at);
        let end = if removed.is_empty() {
            start
        } else {
            // The range the edit took out of the prose, mapped back to the
            // Markdown. Asking for the position *after* the range is what keeps
            // this on a character boundary: the byte in the middle of a Chinese
            // character has no place in the source, and slicing there panics.
            let mut end = shown.source_at(removed.end).max(start);
            // A run whose whole content was removed goes away with its markers.
            // Leaving `****` behind would be punctuation nobody typed.
            if let Some(element) = shown
                .elements
                .iter()
                .find(|element| shown.rendered_range(element) == Some((removed.start, removed.end)))
            {
                end = element.span.end.max(start);
            }
            end.min(shown.source_len)
        };
        let mut text = self.blocks[index].text.to_string();
        if start > end || end > text.len() {
            return;
        }
        text.replace_range(start..end, inserted);

        // Enter arrives as a line break in the field, because that is what a
        // code editor does with the key. In Markdown a break inside a paragraph
        // is not a paragraph, so the break becomes the seam between two blocks
        // instead: everything before it stays here, everything after it moves
        // into a block of its own. Pasting several lines does the same thing
        // once per line, and leaves the caret after the last of them.
        if inserted.contains('\n') {
            // Windows clipboards hand over `\r\n`; the model speaks `\n`.
            let whole = text.replace("\r\n", "\n");
            let parts: Vec<String> = whole.split('\n').map(str::to_string).collect();
            self.remember(self.blocks[index].id);
            self.dirty = true;
            let tail = parts.get(1).cloned().unwrap_or_default();
            self.break_block(index, parts[0].clone(), tail, window, cx);
            let mut at = index + 1;
            for line in parts.iter().skip(2) {
                let block = self.make_block(
                    window,
                    cx,
                    PlainBlock::new(BlockKind::Paragraph, line.clone()),
                );
                at += 1;
                self.blocks.insert(at, block);
            }
            if parts.len() > 2 {
                let state = self.blocks[at].state.clone();
                let end = state.read(cx).value().len();
                state.update(cx, |state, cx| state.set_selected_range(end..end, cx));
                self.focus_block(at, window, cx);
            }
            self.schedule_autosave(window, cx);
            cx.notify();
            return;
        }

        let caret = (start + inserted.len()).min(text.len());

        self.remember(self.blocks[index].id);
        self.blocks[index].text = text.clone().into();
        self.dirty = true;

        // Put the caret back where the typing left it, in whatever the field
        // should be showing now: a run it just left has to close again, and one
        // it just entered has to open.
        let (next, placed) = view_at(&text, caret);
        let caret = next.source_point(placed);
        self.blocks[index].shown_open = inline::render(&text)
            .element_at(caret)
            .map(|element| element.span.clone());
        self.blocks[index].shown = Some(next.clone());
        if next.text != edited {
            let value = next.text.clone();
            state.update(cx, |state, cx| {
                state.set_value(value, window, cx);
                state.set_selected_range(placed..placed, cx);
            });
        }
        self.apply_shortcut(index, window, cx);
        self.schedule_autosave(window, cx);
        cx.notify();
    }

    /// Turn `# `, `- `, `> ` … typed at the start of a paragraph into a block
    /// of that type, the way Typora does.
    fn apply_shortcut(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocks[index].kind != BlockKind::Paragraph {
            return;
        }
        let text = self.blocks[index].text.to_string();
        let Some((kind, rest)) = detect_shortcut(&text) else {
            return;
        };

        if kind.is_code() {
            // A fence only opens once there is something after the backticks,
            // so a bare ``` stays as text while the language is being typed.
            self.blocks[index].text = rest.clone().into();
            self.blocks[index].kind = kind;
            self.rebuild_states(index, window, cx);
            self.dirty = true;
            cx.notify();
            return;
        }

        let state = self.blocks[index].state.clone();
        let prose = inline::render(&rest).text;
        state.update(cx, |state, cx| {
            state.set_value(prose.clone(), window, cx);
            let end = prose.len();
            state.set_selected_range(end..end, cx);
        });
        self.blocks[index].shown = None;
        self.blocks[index].kind = kind;
        self.blocks[index].text = rest.into();
        self.dirty = true;
        cx.notify();
    }

    /// Enter inside a block: split it at the caret and continue the structure.
    fn split_block(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.blocks[index].text.to_string();
        let shown = self
            .blocks[index]
            .shown
            .clone()
            .unwrap_or_else(|| inline::render(&text));
        let caret = self.blocks[index].state.read(cx).selected_range().end;
        let caret = boundary(&text, shown.source_point(caret).min(text.len()));
        let (head, tail) = text.split_at(caret);
        self.break_block(index, head.to_string(), tail.to_string(), window, cx);
    }

    /// Break one block in two at a point already resolved to a byte of the
    /// Markdown: `head` stays where it is, `tail` moves into a block of its
    /// own. The structure carries over where it can — a bullet continues as a
    /// bullet, a quote as a quote — and a fence or a divider on its own line is
    /// recognised from the head.
    fn break_block(
        &mut self,
        index: usize,
        head: String,
        tail: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let kind = self.blocks[index].kind.clone();

        // ``` or ```rust on its own line opens a code fence.
        if kind == BlockKind::Paragraph {
            if let Some(language) = fence_language(&head) {
                if tail.trim().is_empty() {
                    self.blocks[index].kind = BlockKind::Code(language);
                    self.blocks[index].text = "".into();
                    self.rebuild_states(index, window, cx);
                    self.dirty = true;
                    return;
                }
            }
        }

        // `---` on its own line becomes a divider.
        if kind == BlockKind::Paragraph && is_divider(&head) && tail.trim().is_empty() {
            self.blocks[index].kind = BlockKind::Rule;
            self.blocks[index].text = "".into();
            self.rebuild_states(index, window, cx);
            let block = self.make_block(window, cx, PlainBlock::new(BlockKind::Paragraph, ""));
            self.blocks.insert(index + 1, block);
            self.focus_block(index + 1, window, cx);
            self.dirty = true;
            return;
        }

        // Pressing Enter on an empty list item leaves the list.
        if kind.is_marked() && head.trim().is_empty() && tail.trim().is_empty() {
            self.set_block_kind(index, BlockKind::Paragraph, window, cx);
            return;
        }

        let next_kind = kind.continue_on_enter().unwrap_or(BlockKind::Paragraph);

        self.blocks[index].text = head.clone().into();
        let state = self.blocks[index].state.clone();
        let prose = inline::render(&head).text;
        state.update(cx, |state, cx| {
            state.set_value(prose.clone(), window, cx);
            let end = prose.len();
            state.set_selected_range(end..end, cx);
        });
        self.blocks[index].shown = None;

        let block = self.make_block(window, cx, PlainBlock::new(next_kind, tail));
        self.blocks.insert(index + 1, block);
        self.focus_block(index + 1, window, cx);
        self.dirty = true;
        cx.notify();
    }

    fn set_block_kind(
        &mut self,
        index: usize,
        kind: BlockKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remember(self.blocks[index].id);
        let was_code = self.blocks[index].is_code();
        let is_code = kind.is_code();
        self.blocks[index].kind = kind.clone();
        if was_code != is_code {
            self.rebuild_states(index, window, cx);
        } else if let BlockKind::Code(Some(language)) = &kind {
            let language = language.clone();
            let state = self.blocks[index].state.clone();
            state.update(cx, |state, cx| state.set_highlighter(language, cx));
        }
        self.dirty = true;
        cx.notify();
    }

    /// Recreate the fields of a block, keeping its id and its text. Used when a
    /// block turns into a code fence (or back), because a fence is highlighted
    /// by its language and everything else by the inline Markdown runs.
    fn rebuild_states(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.blocks[index].id;
        let kind = self.blocks[index].kind.clone();
        let text = self.blocks[index].text.to_string();
        let (state, lang, plain, styles, subscriptions) =
            self.build_state(id, kind, text, window, cx);
        self.blocks[index].state = state;
        self.blocks[index].styles = styles;
        self.blocks[index].lang = lang;
        self.blocks[index].plain = plain;
        self.blocks[index]._subscriptions = subscriptions;
        self.focus_block(index, window, cx);
        self.dirty = true;
        cx.notify();
    }

    fn delete_block(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocks.len() <= 1 {
            return;
        }
        self.remember(self.blocks[index].id);
        let block = self.blocks.remove(index);
        if self.active_id == Some(block.id) {
            self.active_id = None;
        }
        let target = index.saturating_sub(1);
        self.focus_block(target, window, cx);
        self.dirty = true;
        cx.notify();
    }

    // ------------------------------------------------------------- commands

    fn wrap_selection(
        &mut self,
        left: &str,
        right: &str,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.active_index() else {
            self.hint = "先点进一个段落再排版".into();
            cx.notify();
            return;
        };
        if self.blocks[index].kind.is_rule() || self.blocks[index].is_code() {
            return;
        }
        let state = self.blocks[index].state.clone();
        let selected = state.read(cx).selected_text().to_string();
        let body = if selected.is_empty() {
            placeholder.to_string()
        } else {
            selected
        };
        state.update(cx, |state, cx| {
            state.replace(format!("{left}{body}{right}"), window, cx)
        });
        self.dirty = true;
        cx.notify();
    }

    fn apply_active_kind(&mut self, kind: BlockKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_index() else {
            self.hint = "先点进一个段落再选择样式".into();
            cx.notify();
            return;
        };
        let next = if self.blocks[index].kind == kind {
            BlockKind::Paragraph
        } else {
            kind
        };
        self.set_block_kind(index, next, window, cx);
    }

    fn new_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settle_before_switch(window, cx) {
            return;
        }
        let block = self.make_block(window, cx, PlainBlock::new(BlockKind::Paragraph, ""));
        self.blocks = vec![block];
        self.path = None;
        self.sample = false;
        self.dirty = false;
        self.source_mode = false;
        self.focus_first(window, cx);
        self.hint = "新文档".into();
        cx.notify();
    }

    /// Take care of unsaved work before the page is replaced. A document that
    /// already has a name is written back first, so clicking another file in
    /// the side panel never silently throws an edit away — the old behaviour
    /// simply refused to switch, which read as a frozen window. Only a draft
    /// that has never been saved can hold the switch up, because there is no
    /// file to write it to.
    fn settle_before_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.dirty {
            return true;
        }
        let Some(path) = self.path.clone() else {
            if self.is_blank() {
                self.dirty = false;
                return true;
            }
            self.hint = "这份文档还没有名字，先 Ctrl+S 保存一下".into();
            cx.notify();
            return false;
        };
        let source = self.current_source(cx);
        self.write_to(path, source, window, cx);
        !self.dirty
    }

    /// The document as it would be written out, in whichever mode it is in.
    fn current_source(&self, cx: &Context<Self>) -> String {
        if self.source_mode {
            self.source.read(cx).value().to_string()
        } else {
            model::serialize(&self.plain_blocks())
        }
    }

    /// A document with nothing in it is not worth protecting.
    fn is_blank(&self) -> bool {
        self.blocks.len() == 1
            && self.blocks[0].kind == BlockKind::Paragraph
            && self.blocks[0].text.trim().is_empty()
    }

    fn open_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settle_before_switch(window, cx) {
            return;
        }
        // A native dialog pumps the OS message loop, so it must not run while
        // GPUI holds a mutable borrow of the app. Run it off-thread instead.
        let task = cx.background_executor().spawn(async {
            rfd::FileDialog::new()
                .add_filter("Markdown", &["md", "markdown", "txt"])
                .pick_file()
        });

        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = task.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.load_from_path(path, window, cx))
                .ok();
        })
        .detach();
    }

    fn load_from_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match std::fs::read_to_string(&path) {
            Ok(source) => {
                // Opened from outside any workspace — the file list follows
                // the document into its own folder.
                self.adopt_parent(&path);
                self.set_document(&source, Some(path), window, cx)
            }
            Err(err) => {
                self.hint = format!("打开失败：{err}").into();
                cx.notify();
            }
        }
    }

    fn save_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.current_source(cx);

        let Some(path) = self.path.clone() else {
            // A page typed on before any note was picked is saved inside the
            // folder that is open, not wherever the dialog last pointed.
            let folder = self.root.clone();
            let task = cx.background_executor().spawn(async move {
                let mut dialog = rfd::FileDialog::new()
                    .add_filter("Markdown", &["md"])
                    .set_file_name("untitled.md");
                if let Some(folder) = folder {
                    dialog = dialog.set_directory(folder);
                }
                dialog.save_file()
            });

            cx.spawn_in(window, async move |this, cx| {
                let Some(path) = task.await else {
                    return;
                };
                this.update_in(cx, |this, window, cx| this.write_to(path, source, window, cx))
                    .ok();
            })
            .detach();
            return;
        };

        self.write_to(path, source, window, cx);
    }

    /// Turn "write it back a moment after you stop typing" on or off, and
    /// remember the choice across restarts.
    fn toggle_auto_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.auto_save = !self.settings.auto_save;
        if let Err(err) = self.settings.save() {
            self.hint = format!("设置保存失败：{err}").into();
        }
        if self.settings.auto_save {
            self.hint = if self.path.is_some() {
                "自动保存已开启 · 停手一秒后写回磁盘".into()
            } else {
                "自动保存已开启 · 先 Ctrl+S 指定位置".into()
            };
            self.schedule_autosave(window, cx);
        } else {
            self.hint = "自动保存已关闭".into();
        }
        cx.notify();
    }

    /// Restart the idle timer. Every edit stamps a new deadline, so a burst of
    /// typing costs one write instead of one per keystroke.
    fn schedule_autosave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.auto_save || self.path.is_none() {
            return;
        }
        self.autosave_at = Some(Instant::now());
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            this.update_in(cx, |this, window, cx| this.autosave_if_idle(window, cx))
                .ok();
        })
        .detach();
    }

    fn autosave_if_idle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stamped) = self.autosave_at else {
            return;
        };
        if !self.settings.auto_save {
            return;
        }
        // Timers wake up a fraction early often enough to matter. Waiting out
        // the rest beats giving up — an aborted check used to be the last one
        // in the chain, which is why nothing ever reached the disk.
        let elapsed = stamped.elapsed();
        if elapsed < AUTOSAVE_DELAY {
            let remaining = AUTOSAVE_DELAY - elapsed;
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(remaining).await;
                this.update_in(cx, |this, window, cx| this.autosave_if_idle(window, cx))
                    .ok();
            })
            .detach();
            return;
        }
        if !self.dirty {
            return;
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        let source = if self.source_mode {
            self.source.read(cx).value().to_string()
        } else {
            model::serialize(&self.plain_blocks())
        };
        if std::fs::write(&path, source).is_ok() {
            self.dirty = false;
            self.autosave_at = None;
            self.hint = format!("已自动保存 {}", file_name(&path)).into();
            if let Some(title) = self.window_title() {
                window.set_window_title(&title);
            }
            cx.notify();
        }
    }

    fn write_to(
        &mut self,
        path: PathBuf,
        source: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match std::fs::write(&path, source) {
            Ok(()) => {
                self.hint = format!("已保存 {}", file_name(&path)).into();
                self.last_dir = path.parent().map(|dir| dir.to_path_buf());
                self.path = Some(path);
                self.dirty = false;
                // A freshly created file has to appear in the folder listing.
                self.folder_scan_at = None;
                if let Some(title) = self.window_title() {
                    window.set_window_title(&title);
                }
            }
            Err(err) => self.hint = format!("保存失败：{err}").into(),
        }
        cx.notify();
    }

    fn set_document(
        &mut self,
        source: &str,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut blocks: Vec<Block> = Vec::new();
        for plain in model::parse(source) {
            blocks.push(self.make_block(window, cx, plain));
        }
        if blocks.is_empty() {
            blocks.push(self.make_block(window, cx, PlainBlock::new(BlockKind::Paragraph, "")));
        }

        self.blocks = blocks;
        if let Some(dir) = path.as_ref().and_then(|path| path.parent()) {
            self.last_dir = Some(dir.to_path_buf());
        }
        self.path = path;
        self.sample = false;
        self.dirty = false;
        self.source_mode = false;
        self.active_id = None;
        self.folder_scan_at = None;
        // The blocks are new, so the ids the link measurement remembers are
        // meaningless now.
        self.link_asked.borrow_mut().clear();
        self.focus_first(window, cx);
        if let Some(title) = self.window_title() {
            window.set_window_title(&title);
        }
        self.hint = "已打开".into();
        cx.notify();
    }

    fn toggle_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.source_mode {
            let text = self.source.read(cx).value().to_string();
            self.set_document(&text, self.path.clone(), window, cx);
            self.hint = "回到所见即所得".into();
        } else {
            let text = model::serialize(&self.plain_blocks());
            let source = self.source.clone();
            source.update(cx, |state, cx| {
                state.set_value(text, window, cx);
                state.focus(window, cx);
            });
            self.source_mode = true;
            self.active_id = None;
            self.hint = "源码模式 · Ctrl+/ 返回".into();
        }
        cx.notify();
    }

    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = if cx.theme().is_dark() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        Theme::change(next, Some(window), cx);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let command = keystroke.modifiers.control || keystroke.modifiers.platform;
        let shift = keystroke.modifiers.shift;
        let alt = keystroke.modifiers.alt;
        let key = keystroke.key.to_lowercase();

        // The picture viewer is the topmost thing on screen. While it is up it
        // owns every key, so nothing typed at it reaches the document behind —
        // and the keys that move around inside it do that instead.
        if self.viewer.is_some() {
            let viewport = window.viewport_size();
            let middle = point(viewport.width / 2.0, viewport.height / 2.0);
            match key.as_str() {
                "escape" => self.close_viewer(cx),
                "left" => self.viewer_step(-1, cx),
                "right" => self.viewer_step(1, cx),
                "=" | "+" | "add" => self.viewer_zoom(1.25, middle, viewport, cx),
                "-" | "_" | "subtract" => self.viewer_zoom(0.8, middle, viewport, cx),
                "0" => self.viewer_reset(cx),
                _ => {}
            }
            cx.stop_propagation();
            return;
        }

        // The language list is the popup half of the field that holds the
        // keyboard: Escape puts it away, Enter takes the row it is on, and Up
        // and Down walk it (as actions, so they never reach this far).
        if self.lang_menu.is_some() {
            match key.as_str() {
                "escape" => {
                    self.close_lang_menu(cx);
                    cx.stop_propagation();
                    return;
                }
                "enter" => {
                    self.accept_lang_row(window, cx);
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }

        // The file menu is dismissed by Escape, and a prompt owns the keyboard
        // while it is up — including Enter, which is its "确定".
        if self.menu.is_some() && key == "escape" {
            self.close_menu(cx);
            cx.stop_propagation();
            return;
        }
        if self.prompt.is_some() {
            if key == "escape" {
                self.close_prompt(window, cx);
                cx.stop_propagation();
                return;
            }
            if key == "enter" && !shift && !command && !alt {
                self.commit_prompt(window, cx);
                cx.stop_propagation();
                return;
            }
            // Everything else belongs to the field inside the prompt.
            return;
        }

        // The close question owns the keyboard while it is on screen: Enter
        // saves, Escape calls the whole thing off, and nothing else reaches the
        // page underneath.
        if self.close_prompt {
            match key.as_str() {
                "enter" => self.save_and_close(window, cx),
                "escape" => self.cancel_close(window, cx),
                _ => {}
            }
            cx.stop_propagation();
            return;
        }

        // The settings dialog owns the keyboard while it is open; only Escape
        // still belongs to the document.
        if self.show_settings && key != "escape" {
            return;
        }
        // Enter inside a paragraph is the field's to report: the editor turns
        // the key into a line break, and `apply_paragraph_edit` reads that
        // break as the seam of a new block. This arm is what is left for the
        // case where the document itself holds the keyboard.
        if key == "enter" && !command && !alt && !shift {
            // A cell owns its Enter: the field reads it as "next cell" and the
            // table stays whole. Splitting the block here would cut the grid in
            // two under the caret — the header row on one side, the rows on the
            // other, neither of them a table any more.
            if self.table_edit.is_some() {
                return;
            }
            if let Some(index) = self.active_index() {
                if !self.blocks[index].is_code() && self.blocks[index].plain.is_none() {
                    self.remember(self.blocks[index].id);
                    self.split_block(index, window, cx);
                    self.schedule_autosave(window, cx);
                    cx.stop_propagation();
                    return;
                }
            }
        }

        // F-keys are handled before anything else so they work everywhere.
        match key.as_str() {
            "f8" => {
                self.focus_mode = !self.focus_mode;
                self.hint = if self.focus_mode {
                    "专注模式 · F8 退出".into()
                } else {
                    "已退出专注模式".into()
                };
                cx.notify();
                cx.stop_propagation();
                return;
            }
            "f11" => {
                window.toggle_fullscreen();
                cx.stop_propagation();
                return;
            }
            _ => {}
        }

        // Rows are added and removed from the keyboard, where the caret already
        // is. Both keys are answered before the branches below, and before the
        // field gets them: Ctrl+Enter would reach the cell as a plain Enter and
        // walk to the next cell, and Ctrl+Shift+Backspace would fall through
        // the shifted table and be dropped.
        if command && (key == "enter" || (key == "backspace" && shift)) {
            if let Some(edit) = self.table_edit.as_ref() {
                let (block, row, column) = (edit.block, edit.row, edit.column);
                if key == "enter" {
                    self.insert_table_row_below(block, row, column, window, cx);
                } else {
                    self.delete_table_row(block, row, column, window, cx);
                }
                cx.stop_propagation();
                return;
            }
            if key == "enter" {
                self.hint = "先把光标放进表格的单元格里".into();
                cx.notify();
            }
        }

        if command && shift {
            match key.as_str() {
                "z" => {
                    self.redo(window, cx);
                    cx.stop_propagation();
                    return;
                }
                "s" => {
                    self.save_as(window, cx);
                    cx.stop_propagation();
                }
                "k" => {
                    self.apply_active_kind(BlockKind::Code(None), window, cx);
                    cx.stop_propagation();
                }
                "q" => {
                    self.apply_active_kind(BlockKind::Quote, window, cx);
                    cx.stop_propagation();
                }
                "m" => {
                    self.toggle_math_block(window, cx);
                    cx.stop_propagation();
                }
                "[" => {
                    self.apply_active_kind(BlockKind::Ordered, window, cx);
                    cx.stop_propagation();
                }
                "]" => {
                    self.apply_active_kind(BlockKind::Bullet, window, cx);
                    cx.stop_propagation();
                }
                "x" => {
                    self.apply_active_kind(BlockKind::Todo(false), window, cx);
                    cx.stop_propagation();
                }
                "i" => {
                    self.insert_image(window, cx);
                    cx.stop_propagation();
                }
                "o" => {
                    self.open_folder(window, cx);
                    cx.stop_propagation();
                }
                "u" => {
                    self.upload_all_images(window, cx);
                    cx.stop_propagation();
                }
                "`" | "~" | "backquote" => {
                    // The selection, not the line: a shortcut that turns the
                    // paragraph into a code block on the way to marking three
                    // words is a shortcut nobody can aim.
                    self.wrap_selection("`", "`", "code", window, cx);
                    cx.stop_propagation();
                }
                "e" => {
                    self.wrap_selection("`", "`", "code", window, cx);
                    cx.stop_propagation();
                }
                "l" => {
                    self.toggle_sidebar(cx);
                    cx.stop_propagation();
                }
                _ => {}
            }
            return;
        }

        if command {
            match key.as_str() {
                // Ctrl+Shift+` arrives as `~` with the shift bit already folded
                // into the character on Windows, so this arm is the one that
                // actually fires for the shortcut. It sits here, not in the
                // shifted branch, because `keystroke.modifiers.shift` is false
                // for it: the layout produced `~` and the modifier did not
                // survive the trip. It wraps the selection in inline code, the
                // same thing Ctrl+E does.
                "`" | "~" | "backquote" => {
                    self.wrap_selection("`", "`", "code", window, cx);
                    cx.stop_propagation();
                }
                // The keymap turns Ctrl+Z into an action while a field has
                // focus, and the capture listener above takes it from there.
                // With nothing focused the keystroke never becomes an action,
                // so it has to be handled here as well.
                "z" => {
                    self.undo(window, cx);
                    cx.stop_propagation();
                }
                "y" => {
                    self.redo(window, cx);
                    cx.stop_propagation();
                }
                "s" => {
                    self.save_document(window, cx);
                    cx.stop_propagation();
                }
                "o" => {
                    self.open_document(window, cx);
                    cx.stop_propagation();
                }
                "n" => {
                    self.new_document(window, cx);
                    cx.stop_propagation();
                }
                "w" => {
                    self.request_close(window, cx);
                    cx.stop_propagation();
                }
                "," => {
                    self.open_settings(SettingsTab::General, window, cx);
                    cx.stop_propagation();
                }
                "/" => {
                    self.toggle_source(window, cx);
                    cx.stop_propagation();
                }
                "b" => {
                    self.wrap_selection("**", "**", "加粗", window, cx);
                    cx.stop_propagation();
                }
                "i" => {
                    self.wrap_selection("*", "*", "斜体", window, cx);
                    cx.stop_propagation();
                }
                "u" => {
                    self.wrap_selection("<u>", "</u>", "下划线", window, cx);
                    cx.stop_propagation();
                }
                "k" => {
                    self.wrap_selection("[", "](https://)", "链接文字", window, cx);
                    cx.stop_propagation();
                }
                "\\" => {
                    self.clear_format(window, cx);
                    cx.stop_propagation();
                }
                "t" => {
                    self.insert_table(window, cx);
                    cx.stop_propagation();
                }
                "=" => {
                    self.shift_heading(-1, window, cx);
                    cx.stop_propagation();
                }
                "-" => {
                    self.shift_heading(1, window, cx);
                    cx.stop_propagation();
                }
                "l" => {
                    self.select_current_line(cx);
                    cx.stop_propagation();
                }
                "home" => {
                    self.focus_block(0, window, cx);
                    cx.stop_propagation();
                }
                "end" => {
                    if !self.blocks.is_empty() {
                        self.focus_block(self.blocks.len() - 1, window, cx);
                    }
                    cx.stop_propagation();
                }
                "1" | "2" | "3" | "4" | "5" | "6" => {
                    let level = key.as_bytes()[0] - b'0';
                    self.apply_active_kind(BlockKind::Heading(level), window, cx);
                    cx.stop_propagation();
                }
                "0" => {
                    self.apply_active_kind(BlockKind::Paragraph, window, cx);
                    cx.stop_propagation();
                }
                // A screenshot on the clipboard becomes an image block; text
                // pastes fall through to the field itself.
                "v" => {
                    if !self.show_settings && self.paste_clipboard_image(window, cx) {
                        cx.stop_propagation();
                    }
                }
                _ => {}
            }
            return;
        }

        if alt && shift {
            if key == "5" {
                self.wrap_selection("~~", "~~", "删除线", window, cx);
                cx.stop_propagation();
            }
            return;
        }

        if alt && matches!(key.as_str(), "up" | "down") {
            let delta = if key == "up" { -1 } else { 1 };
            self.move_block(delta, window, cx);
            cx.stop_propagation();
            return;
        }

        // Up/Down at the first or last line of a block continue into the
        // neighbouring block instead of dead-ending.
        if matches!(key.as_str(), "up" | "down") {
            if self.arrow_step(key == "up", window, cx) {
                cx.stop_propagation();
            }
            return;
        }

        match key.as_str() {
            "escape" => {
                if self.show_settings {
                    self.close_settings(window, cx);
                } else if self.focus_mode {
                    self.focus_mode = false;
                    cx.notify();
                } else if self.active_id.is_some() {
                    self.active_id = None;
                    cx.notify();
                }
            }
            "backspace" => {
                let Some(index) = self.active_index() else {
                    return;
                };
                if self.blocks[index].kind.is_rule() {
                    self.delete_block(index, window, cx);
                    cx.stop_propagation();
                    return;
                }
                let state = self.blocks[index].state.clone();
                let empty =
                    self.blocks[index].text.is_empty() && state.read(cx).selected_range().is_empty();
                if empty {
                    self.delete_block(index, window, cx);
                    cx.stop_propagation();
                }
            }
            _ => {}
        }
    }

    /// Save to a new file, always asking for the path.
    fn save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_as_then(window, cx, |_, _, _| {});
    }

    /// Ask the file manager where the document should go, write it there, and
    /// hand the result to `then`.
    ///
    /// The dialog is the only way to name a file that has no path yet, so both
    /// "另存为" and the save half of the close question come through here; the
    /// close question needs the follow-up, because whether the window may shut
    /// depends on the write having gone through.
    fn save_as_then<F>(&mut self, window: &mut Window, cx: &mut Context<Self>, then: F)
    where
        F: FnOnce(&mut MeowDown, &mut Window, &mut Context<MeowDown>) + 'static,
    {
        let source = if self.source_mode {
            self.source.read(cx).value().to_string()
        } else {
            model::serialize(&self.plain_blocks())
        };
        let suggested = self
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled.md".to_string());

        // "另存为" starts where the note lives, or in the folder that is open.
        let folder = self
            .path
            .as_ref()
            .and_then(|path| path.parent())
            .or(self.root.as_deref())
            .map(|dir| dir.to_path_buf());

        let task = cx.background_executor().spawn(async move {
            let mut dialog = rfd::FileDialog::new()
                .add_filter("Markdown", &["md"])
                .set_file_name(suggested);
            if let Some(folder) = folder {
                dialog = dialog.set_directory(folder);
            }
            dialog.save_file()
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = task.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.write_to(path, source, window, cx);
                then(this, window, cx);
            })
            .ok();
        })
        .detach();
    }

    // ---------------------------------------------------------------- closing

    /// Whether closing the window would throw away something typed.
    ///
    /// A page that was typed into and emptied again is not worth a question:
    /// there is nothing in it that was not in a blank document already.
    fn unsaved(&self) -> bool {
        self.dirty && !self.is_blank()
    }

    /// The platform is about to close the window. `true` lets it go.
    ///
    /// This runs inside the platform's own close handling, so the window must
    /// not be taken down from here — the answer is only "yes, go" or "no, ask
    /// first". Everything that closes the window *after* an answer goes
    /// through `window.remove_window()` on a normal click instead.
    fn before_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.unsaved() {
            return true;
        }
        self.ask_before_closing(window, cx);
        false
    }

    /// Close, or ask first. Used by the 退出 menu item and Ctrl+W, which are
    /// ours and never reach the platform's close handler.
    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.unsaved() {
            window.remove_window();
            return;
        }
        self.ask_before_closing(window, cx);
    }

    /// Put the question on screen and take the keyboard away from the page.
    ///
    /// The card becomes the focused node, and that is not cosmetic: with the
    /// caret still in a paragraph, Enter arrives as text and the field turns it
    /// into a line break before any key listener sees it — a "save this?" Enter
    /// would quietly edit the document it is asking about. Focus also keeps the
    /// keys on a path this view is on, which is where the answer is read.
    fn ask_before_closing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_prompt = true;
        window.focus(&self.close_focus, cx);
        cx.notify();
    }

    /// 保存: write the document out, then let the window go.
    ///
    /// A document with a path is written straight back to it. One without is
    /// sent through the file manager first, and only closes once the file
    /// exists — a cancelled dialog leaves the window and the text where they
    /// were.
    fn save_and_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_prompt = false;
        if let Some(path) = self.path.clone() {
            let source = self.current_source(cx);
            self.write_to(path, source, window, cx);
            if !self.dirty {
                window.remove_window();
            }
            return;
        }
        self.save_as_then(window, cx, |this, window, _cx| {
            if !this.dirty {
                window.remove_window();
            }
        });
    }

    /// 丢弃: drop what was typed and close.
    fn discard_and_close(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        self.close_prompt = false;
        window.remove_window();
    }

    /// 取消: forget the question and keep editing.
    fn cancel_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_prompt = false;
        self.return_to_document(window, cx);
        cx.notify();
    }

    // --------------------------------------------------------------- helpers

    fn plain_blocks(&self) -> Vec<PlainBlock> {
        self.blocks
            .iter()
            .map(|block| PlainBlock::new(block.kind.clone(), block.text.to_string()))
            .filter(|block| !(block.text.trim().is_empty() && block.kind == BlockKind::Paragraph))
            .collect()
    }

    fn document_text(&self) -> String {
        self.blocks
            .iter()
            .map(|block| block.text.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn window_title(&self) -> Option<String> {
        let name = self
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            // A folder with no note picked yet is titled after the folder: it
            // is what is open, and "未命名.md" would suggest a document is.
            .or_else(|| {
                self.root
                    .as_ref()
                    .and_then(|root| root.file_name())
                    .filter(|_| self.path.is_none())
            })
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "未命名.md".to_string());
        Some(format!(
            "{}{} — MeowDown",
            if self.dirty { "• " } else { "" },
            name
        ))
    }

    /// Order number for an ordered list item, counted inside its own run.
    fn ordinal_at(&self, index: usize) -> usize {
        let mut ordinal = 1;
        let mut cursor = index;
        while cursor > 0 {
            cursor -= 1;
            if self.blocks[cursor].kind == BlockKind::Ordered {
                ordinal += 1;
            } else {
                break;
            }
        }
        ordinal
    }

    fn outline(&self) -> Vec<(u64, u8, SharedString)> {
        self.blocks
            .iter()
            .filter_map(|block| match block.kind {
                // The outline is a list of names, not prose: `## 见 [文档](url)`
                // reads as `见 文档` here — the same words the heading itself
                // shows, with none of the syntax that puts them in a style.
                BlockKind::Heading(level) => Some((
                    block.id,
                    level,
                    SharedString::from(model::strip_inline_markup(&block.text)),
                )),
                _ => None,
            })
            .collect()
    }

    // ---------------------------------------------------------- sidebar lists

    fn set_sidebar_tab(&mut self, tab: SidebarTab, cx: &mut Context<Self>) {
        if self.sidebar_tab != tab {
            // Slide from wherever the thumb is right now, so clicking twice in
            // quick succession does not jump back to the old slot first.
            let target = tab_slot(tab);
            if (self.tab_pos - target).abs() > 0.001 {
                self.tab_anim = Some(Slide {
                    from: self.tab_pos,
                    to: target,
                    start: Instant::now(),
                    duration: TAB_GLIDE,
                });
            }
        }
        self.sidebar_tab = tab;
        // Picking a tab from the menu can also be the way the panel comes back
        // after being folded away, so it unfolds with the same glide.
        self.set_sidebar_open(true, cx);
        if tab == SidebarTab::Files {
            self.folder_scan_at = None;
        }
        cx.notify();
    }

    /// Show or fold away the side panel, gliding instead of snapping.
    ///
    /// `sidebar_visible` records the destination at once so a second toggle
    /// knows where the panel was heading; the slide always starts from the
    /// width it has right now, so interrupting one mid-flight turns it around
    /// from where it is rather than jumping to the far end first.
    fn set_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        let target = if open { 1. } else { 0. };
        self.sidebar_visible = open;
        if (self.sidebar_pos - target).abs() > 0.001 {
            self.sidebar_anim = Some(Slide {
                from: self.sidebar_pos,
                to: target,
                start: Instant::now(),
                duration: SIDEBAR_SLIDE,
            });
        }
        cx.notify();
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        let open = !self.sidebar_visible;
        self.set_sidebar_open(open, cx);
    }

    /// The panel's width this frame, in pixels. Advancing the slide here — in
    /// the render pass, driven by `request_animation_frame` — is what keeps it
    /// smooth and free of timers: each frame moves it a little further along
    /// the curve, and the frames stop when it lands.
    fn sidebar_width(&mut self, window: &mut Window) -> f32 {
        if let Some(slide) = self.sidebar_anim {
            let (value, done) = slide.value_at(Instant::now());
            self.sidebar_pos = value;
            if done {
                self.sidebar_pos = slide.to;
                self.sidebar_anim = None;
            } else {
                window.request_animation_frame();
            }
        }
        SIDEBAR_WIDTH * self.sidebar_pos
    }

    /// Switches the panel without gliding: used when the tab changes as a side
    /// effect of opening a folder, where an animation would be noise.
    fn snap_sidebar_tab(&mut self, tab: SidebarTab) {
        self.sidebar_tab = tab;
        self.tab_pos = tab_slot(tab);
        self.tab_anim = None;
    }

    /// Folder the file list is read from. An opened folder wins; otherwise it
    /// is the folder of the open document, then the last one used, then the
    /// working directory.
    fn file_dir(&self) -> PathBuf {
        if let Some(root) = &self.root {
            return root.clone();
        }
        self.path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|dir| dir.to_path_buf())
            .or_else(|| self.last_dir.clone())
            .unwrap_or_else(|| self.doc_dir())
    }

    /// Markdown files directly under [`Self::file_dir`] — one level, never the
    /// whole tree. A note opened on its own counts as a workspace (its folder
    /// is the one it came from), so the flat list is left to a document that
    /// was saved somewhere with no folder opened behind it.
    ///
    /// The folder is re-read at most every [`FOLDER_RESCAN`] so a file added in
    /// Explorer shows up without a refresh button, and immediately when the
    /// document moves. Reading a level is cheap; reading every level was not,
    /// which is why the tree below stays unread until it is opened.
    fn refresh_siblings(&mut self) {
        let dir = self.file_dir();
        let deep = self.root.is_some();
        let moved = self.folder_dir.as_deref() != Some(dir.as_path()) || self.folder_deep != deep;
        let stale = self
            .folder_scan_at
            .map_or(true, |at| at.elapsed() >= FOLDER_RESCAN);
        if moved || stale {
            self.folder_dir = Some(dir.clone());
            self.folder_deep = deep;
            self.folder_scan_at = Some(Instant::now());
            self.folder_files = markdown_files(&dir);
            self.tree = if deep {
                Some(folder_tree(&dir))
            } else {
                None
            };
            // The file being edited should be visible without hunting for it;
            // `reveal_in_tree` also reads back the branches the panel had open,
            // which the rebuild has just emptied.
            if let Some(current) = self.path.clone() {
                self.reveal_in_tree(&current);
            }
        }
    }

    /// Open every folder between the workspace root and `path`, so the current
    /// document shows up in the list — and read those levels, which the tree
    /// does not have until someone asks for them.
    fn reveal_in_tree(&mut self, path: &Path) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.expanded.insert(root.clone());
        let mut cursor = path.parent();
        while let Some(dir) = cursor {
            if !dir.starts_with(&root) {
                break;
            }
            self.expanded.insert(dir.to_path_buf());
            if dir == root {
                break;
            }
            cursor = dir.parent();
        }
        self.sync_tree();
    }

    /// Read every branch the panel is showing open.
    ///
    /// Expansion lives in `expanded`, not in the tree, so a rebuilt tree knows
    /// nothing below its top level: this walks the open paths again and reads
    /// along them. Called after every rebuild and every expand.
    fn sync_tree(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let open: Vec<PathBuf> = self
            .expanded
            .iter()
            .filter(|path| path.starts_with(&root))
            .cloned()
            .collect();
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        tree.load();
        for path in open {
            tree.load_path(&path);
        }
    }

    /// Bring the folder a loose document lives in into the side panel.
    ///
    /// A document with no workspace behind it — opened from Explorer, through
    /// 打开文件, or picked out of a flat list of siblings — carries its folder
    /// with it. A workspace that is already open is the user's own choice and
    /// is left untouched; so is a bare `note.md`, which has no folder worth
    /// listing.
    fn adopt_parent(&mut self, path: &Path) {
        if self.root.is_some() {
            return;
        }
        let Some(dir) = path.parent().filter(|dir| dir.is_dir()) else {
            return;
        };
        self.adopt_root(dir.to_path_buf());
        self.reveal_in_tree(path);
    }

    /// Adopt `dir` as the folder the side panel lists, leaving the document
    /// alone.
    ///
    /// Both ways a document can arrive — a folder named on the command line, a
    /// note opened from Explorer — come through here: the panel reads the
    /// folder as a tree and switches to the file list, because the files beside
    /// the one that opened are what a reader is most likely to want next.
    ///
    /// Only the top level is read. A crowded parent folder — a home directory,
    /// a project root — is exactly where a full walk hurts most, and nothing
    /// below the top is on screen until a folder is opened.
    fn adopt_root(&mut self, dir: PathBuf) {
        self.folder_files = markdown_files(&dir);
        self.folder_dir = Some(dir.clone());
        self.folder_deep = true;
        self.folder_scan_at = Some(Instant::now());
        self.tree = Some(folder_tree(&dir));
        self.last_dir = Some(dir.clone());
        self.root = Some(dir);
        self.snap_sidebar_tab(SidebarTab::Files);
    }

    /// Pick a folder and list every Markdown file inside it, at any depth.
    fn open_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let start = self.root.clone().unwrap_or_else(|| self.file_dir());
        let task = cx.background_executor().spawn(async move {
            rfd::FileDialog::new()
                .set_directory(start)
                .pick_folder()
        });

        cx.spawn_in(window, async move |this, cx| {
            let Some(dir) = task.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.use_folder(dir, window, cx))
                .ok();
        })
        .detach();
    }

    /// Adopt `dir` as the folder the side panel lists.
    fn use_folder(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Top level only: what is below it is read when a folder is opened.
        self.folder_files = markdown_files(&dir);
        self.folder_dir = Some(dir.clone());
        self.folder_deep = true;
        // Let the next repaint read the folder again, which is also what builds
        // the tree and opens the branch the current document lives in.
        self.folder_scan_at = None;
        self.tree = None;
        self.expanded.clear();
        self.last_dir = Some(dir.clone());
        let name = folder_name(&dir);
        self.root = Some(dir);
        self.snap_sidebar_tab(SidebarTab::Files);
        // Opening a folder puts the panel back without a glide: the folder is
        // the thing the user acted on, and watching the panel unfold on top of
        // that just delays the list they asked for.
        self.sidebar_visible = true;
        self.sidebar_pos = 1.;
        self.sidebar_anim = None;
        // The folder is what was opened, so no note is picked for the user.
        // A page still showing the untouched sample gives way to a blank one —
        // otherwise the sample reads as a note that came out of this folder.
        if self.path.is_none() && !self.dirty {
            self.set_document("", None, window, cx);
        }
        self.hint = format!("已打开文件夹 · {name}").into();
        cx.notify();
    }

    /// Switch document from the file list.
    fn open_from_sidebar(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.path.as_deref() == Some(path.as_path()) {
            return;
        }
        if !self.settle_before_switch(window, cx) {
            return;
        }
        self.load_from_path(path, window, cx);
        self.folder_scan_at = None;
    }

    /// Ctrl+click on a link.
    ///
    /// An address can mean three different things, and each is answered where
    /// it can actually be answered:
    ///
    /// * `https:`, `mailto:`, `file:` — outside this window, so the desktop
    ///   gets it; the shell is the only thing that knows which browser the user
    ///   wants;
    /// * `notes/说明.md#安装` — another file of the project, opened here rather
    ///   than in some other editor, because only this window can land on a
    ///   heading inside it;
    /// * `#安装` — a heading of the document already open.
    fn open_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        let url = url.trim();
        if url.is_empty() {
            return;
        }
        let (target, anchor) = match url.split_once('#') {
            Some((target, anchor)) => (target.trim(), Some(anchor.trim())),
            None => (url, None),
        };
        let anchor = anchor
            .filter(|anchor| !anchor.is_empty())
            .map(|anchor| anchor.to_string());

        // A scheme is a place this window has no business opening by itself.
        if has_scheme(target) {
            self.hint = format!("打开 {url}").into();
            open_externally(url);
            cx.notify();
            return;
        }

        // `#一段标题`: a heading of the page on show. Nothing to load.
        if target.is_empty() {
            match anchor.as_deref().and_then(|anchor| self.heading_block(anchor)) {
                Some(id) => {
                    self.hint = "跳到标题".into();
                    self.scroll_to_block(id);
                }
                None => self.hint = format!("这份文档里没有「{}」这个标题", anchor.unwrap_or_default()).into(),
            }
            cx.notify();
            return;
        }

        // A path: relative to the document asking, which is what the author of
        // the link meant by it. A Markdown file is opened here, with the glide
        // to its heading handed to the frame loop; anything else is the
        // desktop's.
        let path = self.doc_dir().join(Path::new(&percent_decode(target)));
        if path.is_file() {
            if is_markdown(&path) || is_plain_text(&path) {
                if !self.settle_before_switch(window, cx) {
                    return;
                }
                self.load_from_path(path, window, cx);
                self.pending_anchor = anchor;
                if self.pending_anchor.is_some() {
                    // The glide itself waits for the frame that knows where the
                    // heading landed — `render_body` keeps asking for one until
                    // the block has been measured. Asking here would not do:
                    // this is a click handler, and a frame can only be
                    // requested from inside a frame.
                    cx.notify();
                }
                return;
            }
            self.hint = format!("打开 {url}").into();
            open_externally(&path.to_string_lossy());
            cx.notify();
            return;
        }

        match resolve_address(url, &self.doc_dir()) {
            Some(address) => {
                self.hint = format!("打开 {url}").into();
                open_externally(&address);
            }
            None => self.hint = format!("找不到这个链接的目标：{url}").into(),
        }
        cx.notify();
    }

    /// The heading a `#安装` fragment names, if this document has one.
    ///
    /// A fragment and a heading never look quite alike: Markdown writes
    /// `#my-heading` for `## My heading`, the address may still carry its
    /// percent-encoding, and Chinese headings are written exactly as they read.
    /// Folding both sides down to letters and digits answers all three.
    fn heading_block(&self, anchor: &str) -> Option<u64> {
        let wanted = anchor_key(anchor);
        if wanted.is_empty() {
            return None;
        }
        self.blocks
            .iter()
            .find(|block| {
                if !matches!(block.kind, BlockKind::Heading(_)) {
                    return false;
                }
                // A heading can be written with syntax the reader never sees:
                // `## 见 [文档](url)` should still answer to `#见-文档`, which
                // is the name the outline lists.
                anchor_key(&block.text) == wanted
                    || anchor_key(&model::strip_inline_markup(&block.text)) == wanted
            })
            .map(|block| block.id)
    }

    // ---------------------------------------------------------------- images

    /// Folder relative image paths are resolved against.
    fn doc_dir(&self) -> PathBuf {
        self.path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|dir| dir.to_path_buf())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Absolute path of a local image reference, `None` for web images.
    fn resolve_image(&self, src: &str) -> Option<PathBuf> {
        if is_remote(src) {
            return None;
        }
        let path = Path::new(src);
        if path.is_absolute() {
            Some(path.to_path_buf())
        } else {
            Some(self.doc_dir().join(path))
        }
    }

    /// The picture behind a web URL, kicking off a download the first time it is
    /// asked for.
    fn remote_image(&mut self, src: &str, cx: &mut Context<Self>) -> RemoteState {
        if let Some(image) = self.remote_images.get(src) {
            return RemoteState::Ready(image.clone());
        }
        if self.remote_failed.contains(src) {
            return RemoteState::Failed;
        }
        if !self.remote_pending.contains(src) {
            self.download_remote(src, cx);
        }
        RemoteState::Pending
    }

    fn download_remote(&mut self, src: &str, cx: &mut Context<Self>) {
        self.remote_pending.insert(src.to_string());
        let url = src.to_string();
        let request = url.clone();
        let task = cx
            .background_executor()
            .spawn(async move { crate::remote::fetch(&request) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.remote_pending.remove(&url);
                match result {
                    Some((bytes, format)) => {
                        let id = crate::remote::image_id(&bytes);
                        let image = Image {
                            format,
                            bytes,
                            id,
                        };
                        this.remote_images.insert(url, Arc::new(image));
                    }
                    None => {
                        this.remote_failed.insert(url);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Note a press on the picture of `block`, so the press that follows it —
    /// the second half of a double click — can be recognised even after the
    /// block has made room for its source and the picture has moved.
    fn note_picture_press(&mut self, block: u64) {
        self.picture_press = Some((block, Instant::now()));
    }

    /// Whether the press just seen completes a double click on the picture of
    /// `block`. The platform only reports a second click within its own
    /// double-click time and distance, so a remembered press of the same block
    /// that is still fresh is the other half of this gesture.
    fn picture_double_clicked(&mut self, block: u64) -> bool {
        let Some((pressed, at)) = self.picture_press.take() else {
            return false;
        };
        pressed == block && at.elapsed() <= Duration::from_millis(1200)
    }

    /// Open the picture of `block` full-window, with every other picture of the
    /// document queued behind it.
    ///
    /// A picture with no source, or one whose path is empty, is not something
    /// to look at: the press on it does what it always did and just shows the
    /// Markdown, so the path can be fixed. Returns whether the viewer opened.
    fn open_viewer(&mut self, block: u64, cx: &mut Context<Self>) -> bool {
        let mut items: Vec<ViewerItem> = Vec::new();
        for candidate in &self.blocks {
            if !candidate.kind.is_image() {
                continue;
            }
            let Some(image) = model::parse_image(&candidate.text) else {
                continue;
            };
            let src = image.src.trim().to_string();
            if src.is_empty() {
                continue;
            }
            items.push(ViewerItem {
                block: candidate.id,
                src,
                alt: image.alt,
            });
        }
        let Some(at) = items.iter().position(|item| item.block == block) else {
            return false;
        };
        self.viewer = Some(Viewer {
            items,
            at,
            zoom: 1.0,
            offset: point(px(0.), px(0.)),
            drag: None,
        });
        cx.notify();
        true
    }

    fn close_viewer(&mut self, cx: &mut Context<Self>) {
        self.viewer = None;
        cx.notify();
    }

    /// Step to the next or the previous picture, wrapping at both ends so the
    /// arrows never turn into dead buttons.
    fn viewer_step(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(viewer) = self.viewer.as_mut() else {
            return;
        };
        viewer.at = wrapped_index(viewer.at, delta, viewer.items.len());
        viewer.zoom = 1.0;
        viewer.offset = point(px(0.), px(0.));
        viewer.drag = None;
        cx.notify();
    }

    /// Zoom by a factor around a point of the window, which stays put: the
    /// picture grows away from the pointer instead of from the middle, which is
    /// what makes zooming into a detail feel like it goes where it is aimed.
    fn viewer_zoom(&mut self, factor: f32, anchor: Point<Pixels>, viewport: Size<Pixels>, cx: &mut Context<Self>) {
        let Some(viewer) = self.viewer.as_mut() else {
            return;
        };
        let zoom = (viewer.zoom * factor).clamp(VIEWER_MIN_ZOOM, VIEWER_MAX_ZOOM);
        if (zoom - viewer.zoom).abs() < f32::EPSILON {
            return;
        }
        let center = point(viewport.width / 2.0, viewport.height / 2.0);
        viewer.offset = zoomed_offset(viewer.offset, anchor, center, zoom / viewer.zoom);
        viewer.zoom = zoom;
        cx.notify();
    }

    /// Back to the picture as it was opened: fitted, in the middle.
    fn viewer_reset(&mut self, cx: &mut Context<Self>) {
        let Some(viewer) = self.viewer.as_mut() else {
            return;
        };
        viewer.zoom = 1.0;
        viewer.offset = point(px(0.), px(0.));
        cx.notify();
    }

    /// Clicking the dimmed room around a picture closes the viewer; clicking
    /// the picture itself does not, so a miss while dragging costs nothing.
    fn viewer_backdrop_clicked(&mut self, at: Point<Pixels>, viewport: Size<Pixels>, cx: &mut Context<Self>) {
        let Some(viewer) = self.viewer.as_ref() else {
            return;
        };
        let src = viewer.items[viewer.at].src.clone();
        let (zoom, offset) = (viewer.zoom, viewer.offset);
        // Without the intrinsic size the picture's own rectangle is unknown, so
        // a click cannot be told from a click on the room around it; Escape and
        // the close button still work.
        let Some(image) = self.image_size(&src) else {
            return;
        };
        let area = viewer_bounds(image, viewport, zoom, offset);
        if at.x < area.left() || at.x > area.right() || at.y < area.top() || at.y > area.bottom() {
            self.close_viewer(cx);
        }
    }

    /// The size the picture was written at, read from the file once and kept.
    fn image_size(&mut self, src: &str) -> Option<Size<f32>> {
        if let Some(size) = self.image_sizes.get(src) {
            return Some(*size);
        }
        let (width, height) = if is_remote(src) {
            let image = self.remote_images.get(src)?;
            image_size_from_bytes(&image.bytes)?
        } else {
            let path = self.resolve_image(src)?;
            image::image_dimensions(path).ok()?
        };
        let size = size(width as f32, height as f32);
        self.image_sizes.insert(src.to_string(), size);
        Some(size)
    }

    fn insert_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.source_mode {
            self.hint = "先退出源码模式再插入图片".into();
            cx.notify();
            return;
        }
        let task = cx.background_executor().spawn(async {
            rfd::FileDialog::new()
                .add_filter("图片", &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"])
                .pick_file()
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = task.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.add_image(path, window, cx))
                .ok();
        })
        .detach();
    }

    /// Insert a picked file: upload it, copy it next to the document, or keep
    /// the absolute path, depending on the settings.
    fn add_image(&mut self, source: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let stem = source
            .file_stem()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "image".to_string());

        if self.settings.auto_upload {
            // Keep a real file in the document's folder first, so the block has
            // something to show while the upload runs — and something to fall
            // back on if it fails.
            let src = match self.settings.image_mode {
                ImageMode::Absolute => source.to_string_lossy().to_string(),
                ImageMode::Relative => self.copy_to_assets(&source, &stem),
            };
            let id = self.insert_image_block(&stem, &src, window, cx);
            self.start_upload(id, source, window, cx);
            return;
        }

        let src = match self.settings.image_mode {
            ImageMode::Absolute => source.to_string_lossy().to_string(),
            ImageMode::Relative => self.copy_to_assets(&source, &stem),
        };
        self.insert_image_block(&stem, &src, window, cx);
        self.hint = format!("已插入图片（{}）", match self.settings.image_mode {
            ImageMode::Relative => "相对路径",
            ImageMode::Absolute => "绝对路径",
        })
        .into();
        cx.notify();
    }

    /// Copy an image into the document's assets folder and return the relative
    /// path written into the Markdown.
    fn copy_to_assets(&self, source: &Path, stem: &str) -> String {
        settings::copy_image_into(
            &self.doc_dir(),
            &self.settings.assets_dir,
            source,
            stem,
        )
        .unwrap_or_else(|| source.to_string_lossy().to_string())
    }

    /// Rows a code block occupies, from the last measurement.
    ///
    /// When the text has changed since that measurement the block is re-shaped
    /// on the spot against the width it had, so typing never waits a frame for
    /// the height to catch up.
    fn code_block_rows(&self, id: u64, index: usize, window: &mut Window) -> usize {
        let text = &self.blocks[index].text;
        let Some(cached) = self.code_rows.borrow().get(&id).copied() else {
            return text.lines().count().max(1);
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(text.as_ref(), &mut hasher);
        if std::hash::Hasher::finish(&hasher) == cached.text_hash {
            return cached.rows;
        }
        wrapped_rows(text, px(cached.width), window)
    }

    fn insert_image_block(
        &mut self,
        alt: &str,
        src: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> u64 {
        let markdown = model::ImageRef {
            alt: alt.to_string(),
            src: src.to_string(),
            title: None,
        }
        .to_markdown();
        let at = self
            .active_index()
            .map(|index| index + 1)
            .unwrap_or(self.blocks.len());
        let block = self.make_block(window, cx, PlainBlock::new(BlockKind::Image, markdown));
        let id = block.id;
        self.blocks.insert(at, block);
        let paragraph = self.make_block(window, cx, PlainBlock::new(BlockKind::Paragraph, ""));
        self.blocks.insert(at + 1, paragraph);
        self.focus_block(at + 1, window, cx);
        self.dirty = true;
        cx.notify();
        id
    }

    fn start_upload(&mut self, id: u64, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.endpoint.trim().is_empty() {
            self.hint = "先在图床设置里填写上传地址".into();
            cx.notify();
            return;
        }
        let settings = self.settings.clone();
        self.uploading.push(id);
        self.hint = "正在上传图片…".into();
        cx.notify();

        let task = cx
            .background_executor()
            .spawn(async move { settings::upload_file(&path, &settings) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.finish_upload(id, result, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn finish_upload(
        &mut self,
        id: u64,
        result: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.uploading.retain(|candidate| *candidate != id);
        match result {
            Ok(url) => {
                if let Some(index) = self.index_of(id) {
                    let alt = model::parse_image(&self.blocks[index].text)
                        .map(|image| image.alt)
                        .unwrap_or_default();
                    let markdown = model::ImageRef {
                        alt,
                        src: url,
                        title: None,
                    }
                    .to_markdown();
                    self.set_block_text(index, &markdown, window, cx);
                }
                self.hint = "图片已上传到图床".into();
            }
            Err(err) => self.hint = format!("上传失败：{err}").into(),
        }
        cx.notify();
    }

    /// Upload every local image referenced by the document, rewriting the
    /// Markdown to point at the returned URLs.
    fn upload_all_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.endpoint.trim().is_empty() {
            self.hint = "先在图床设置里填写上传地址".into();
            cx.notify();
            return;
        }
        let mut jobs: Vec<(u64, PathBuf)> = Vec::new();
        for block in &self.blocks {
            if !block.kind.is_image() {
                continue;
            }
            let Some(image) = model::parse_image(&block.text) else {
                continue;
            };
            let Some(path) = self.resolve_image(&image.src) else {
                continue;
            };
            if path.exists() {
                jobs.push((block.id, path));
            }
        }
        if jobs.is_empty() {
            self.hint = "文档里没有待上传的本地图片".into();
            cx.notify();
            return;
        }

        let settings = self.settings.clone();
        let total = jobs.len();
        self.uploading = jobs.iter().map(|(id, _)| *id).collect();
        self.hint = format!("正在上传 {total} 张图片…").into();
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            let mut results = Vec::new();
            for (id, path) in jobs {
                let outcome = settings::upload_file(&path, &settings);
                results.push((id, outcome));
            }
            results
        });
        cx.spawn_in(window, async move |this, cx| {
            let results = task.await;
            this.update_in(cx, |this, window, cx| {
                let mut done = 0;
                let mut failed = 0;
                let mut first_error: Option<String> = None;
                for (id, outcome) in results {
                    if let Err(err) = &outcome {
                        failed += 1;
                        if first_error.is_none() {
                            first_error = Some(err.clone());
                        }
                    } else {
                        done += 1;
                    }
                    this.finish_upload(id, outcome, window, cx);
                }
                // The per-image message is the one that says *why* — a bare
                // "1 张失败" leaves the user with no idea what to fix.
                this.hint = match (done, first_error) {
                    (_, Some(err)) if failed == 1 => format!("上传失败：{err}").into(),
                    (_, Some(err)) => {
                        format!("上传完成：{done} 张成功，{failed} 张失败（{err}）").into()
                    }
                    _ => format!("上传完成：{done} 张成功").into(),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // -------------------------------------------------------------- settings

    /// Pick one image, upload it with the current settings and report the
    /// result, so the token/endpoint can be checked before trusting them.
    fn test_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read_fields_into_settings(cx);
        if self.settings.endpoint.trim().is_empty() {
            self.hint = "先填写上传地址".into();
            cx.notify();
            return;
        }
        let settings = self.settings.clone();
        self.hint = "正在测试上传…".into();
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            let path = rfd::FileDialog::new()
                .add_filter("图片", &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"])
                .pick_file()?;
            Some(settings::upload_file(&path, &settings))
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await;
            this.update_in(cx, |this, _, cx| {
                this.hint = match outcome {
                    None => "已取消测试".into(),
                    Some(Ok(url)) => format!("测试成功：{url}").into(),
                    Some(Err(err)) => format!("测试失败：{err}").into(),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_settings(&mut self, tab: SettingsTab, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_fields_from_settings(window, cx);
        self.settings_tab = tab;
        self.show_settings = true;
        self.active_id = None;
        cx.notify();
    }

    fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read_fields_into_settings(cx);
        if let Err(err) = self.settings.save() {
            self.hint = format!("设置保存失败：{err}").into();
        }
        self.show_settings = false;
        self.focus_first(window, cx);
        cx.notify();
    }

    fn sync_fields_from_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        let pairs = [
            (self.fields.endpoint.clone(), settings.endpoint),
            (self.fields.token.clone(), settings.token),
            (self.fields.field.clone(), settings.field),
            (self.fields.url_path.clone(), settings.url_path),
            (self.fields.assets_dir.clone(), settings.assets_dir),
        ];
        for (state, value) in pairs {
            state.update(cx, |state, cx| state.set_value(value, window, cx));
        }
    }

    fn read_fields_into_settings(&mut self, cx: &mut Context<Self>) {
        self.settings.token = self.fields.token.read(cx).value().trim().to_string();
        // The endpoint, form field and response path are fixed for the two
        // built-in hosts, so only the custom host reads them back.
        if self.settings.host == ImageHost::Custom {
            self.settings.endpoint = self.fields.endpoint.read(cx).value().trim().to_string();
            self.settings.field = self.fields.field.read(cx).value().trim().to_string();
            self.settings.url_path = self.fields.url_path.read(cx).value().trim().to_string();
        }
        self.settings.assets_dir = self.fields.assets_dir.read(cx).value().trim().to_string();
    }

    // ---------------------------------------------------------- text commands

    fn shift_heading(&mut self, delta: i8, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_index() else {
            return;
        };
        let next = match self.blocks[index].kind {
            BlockKind::Heading(level) => {
                let target = level as i8 + delta;
                if target < 1 {
                    BlockKind::Paragraph
                } else {
                    BlockKind::Heading(target.clamp(1, 6) as u8)
                }
            }
            _ => {
                if delta > 0 {
                    BlockKind::Heading(1)
                } else {
                    BlockKind::Paragraph
                }
            }
        };
        self.set_block_kind(index, next, window, cx);
    }

    /// Point the app at a PicGo.exe the automatic search missed.
    fn pick_picgo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async {
            rfd::FileDialog::new()
                .set_title("选择 PicGo.exe")
                .add_filter("PicGo", &["exe"])
                .add_filter("所有文件", &["*"])
                .pick_file()
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = task.await else {
                return;
            };
            this.update_in(cx, |this, _window, cx| {
                this.settings.picgo_path = path.to_string_lossy().to_string();
                if let Err(err) = this.settings.save() {
                    this.hint = format!("设置没有保存：{err}").into();
                } else {
                    this.hint = "PicGo 位置已保存".into();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn clear_format(&mut self, window: &mut Window, cx: &mut Context<Self>) {        let Some(index) = self.active_index() else {
            self.hint = "先点进一个段落再清除格式".into();
            cx.notify();
            return;
        };
        let state = self.blocks[index].state.clone();
        let selected = state.read(cx).selected_text().to_string();
        let cleaned = model::strip_inline_markup(&selected);
        state.update(cx, |state, cx| {
            state.replace(cleaned, window, cx);
        });
        self.dirty = true;
        cx.notify();
    }

    fn select_current_line(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.active_index() else {
            return;
        };
        let state = self.blocks[index].state.clone();
        let length = state.read(cx).value().len();
        state.update(cx, |state, cx| state.set_selected_range(0..length, cx));
        cx.notify();
    }

    fn move_block(&mut self, delta: i8, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_index() else {
            return;
        };
        let target = index as isize + delta as isize;
        if target < 0 || target as usize >= self.blocks.len() {
            return;
        }
        let target = target as usize;
        self.remember(self.blocks[index].id);
        self.blocks.swap(index, target);
        self.focus_block(target, window, cx);
        self.dirty = true;
        cx.notify();
    }

    // ---------------------------------------------------------------- history

    /// The document exactly as it is right now. Read from the model, not from
    /// the text fields: the model is updated in the same keystroke handler that
    /// calls [`Self::remember`], and the fields have already moved on by then.
    fn step(&self) -> Step {
        Step {
            blocks: self
                .blocks
                .iter()
                .map(|block| (block.kind.clone(), block.text.to_string()))
                .collect(),
            active: self.active_index().unwrap_or(0),
        }
    }

    /// Record what the document looks like before the change that is about to
    /// happen. Keystrokes in one paragraph inside [`UNDO_GROUP`] fold into a
    /// single step, so undo walks back thought by thought, not key by key.
    fn remember(&mut self, block: u64) {
        let now = Instant::now();
        let same_group = matches!(
            self.coalesce,
            Some((last, at)) if last == block && now.duration_since(at) < UNDO_GROUP
        );
        self.coalesce = Some((block, now));
        self.redo_stack.clear();
        if same_group {
            return;
        }
        let step = self.step();
        // Nothing actually differs (a no-op edit), so there is nothing to undo.
        if self.undo_stack.last() == Some(&step) {
            return;
        }
        self.undo_stack.push(step);
        if self.undo_stack.len() > UNDO_DEPTH {
            self.undo_stack.remove(0);
        }
    }

    /// Put the whole document back the way a step recorded it, rebuilding the
    /// fields so every block keeps editing normally afterwards.
    fn restore(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        let mut blocks: Vec<Block> = Vec::new();
        for (kind, text) in step.blocks {
            blocks.push(self.make_block(window, cx, PlainBlock::new(kind, text)));
        }
        if blocks.is_empty() {
            blocks.push(self.make_block(window, cx, PlainBlock::new(BlockKind::Paragraph, "")));
        }
        self.blocks = blocks;
        // Both caches are keyed by block id, and every id just changed.
        self.bounds.borrow_mut().clear();
        self.code_rows.borrow_mut().clear();
        self.source_mode = false;
        self.active_id = None;
        let index = step.active.min(self.blocks.len().saturating_sub(1));
        self.focus_block(index, window, cx);
        self.dirty = true;
        cx.notify();
    }

    fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(previous) = self.undo_stack.pop() else {
            self.hint = "没有可撤销的操作".into();
            cx.notify();
            return;
        };
        let current = self.step();
        if current != previous {
            self.redo_stack.push(current);
        }
        self.coalesce = None;
        self.restore(previous, window, cx);
        self.hint = "已撤销".into();
        self.schedule_autosave(window, cx);
        cx.notify();
    }

    fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(next) = self.redo_stack.pop() else {
            self.hint = "没有可重做的操作".into();
            cx.notify();
            return;
        };
        let current = self.step();
        if current != next {
            self.undo_stack.push(current);
        }
        self.coalesce = None;
        self.restore(next, window, cx);
        self.hint = "已重做".into();
        self.schedule_autosave(window, cx);
        cx.notify();
    }

    // -------------------------------------------------------------- clipboard

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.active_index() else {
            return;
        };
        let (range, text, whole) = {
            let state = self.blocks[index].state.read(cx);
            (
                state.selected_range(),
                state.selected_text().to_string(),
                state.value().to_string(),
            )
        };
        // A chip's backticks are only there to hold its padding open; they are
        // not part of the prose anybody wants to paste somewhere else.
        let text = match &self.blocks[index].shown {
            Some(shown) if shown.text == whole => strip_marks(&text, range.start, &shown.marks),
            _ => text,
        };
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
        self.replace_selection("", window, cx);
    }

    fn select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_index() else {
            return;
        };
        let state = self.blocks[index].state.clone();
        state.update(cx, |state, cx| state.select_all(window, cx));
    }

    fn replace_selection(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_index() else {
            return;
        };
        let text = text.to_string();
        let state = self.blocks[index].state.clone();
        state.update(cx, |state, cx| state.replace(text, window, cx));
        self.dirty = true;
        cx.notify();
    }

    /// Paste text, or an image when the clipboard holds one. Returns whether
    /// something was consumed.
    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.paste_clipboard_image(window, cx) {
            return true;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let Some(text) = item.text() else {
            return false;
        };
        if text.is_empty() {
            return false;
        }
        self.replace_selection(&text, window, cx);
        true
    }

    /// A screenshot on the clipboard becomes an image block right away, then
    /// uploads in the background when a host is configured.
    fn paste_clipboard_image(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let entry = item
            .entries()
            .iter()
            .find(|entry| matches!(entry, ClipboardEntry::Image(_)));
        let Some(ClipboardEntry::Image(shot)) = entry else {
            return false;
        };
        // Windows hands over a DIB, which decodes to a bitmap of several
        // megabytes. Typora saves screenshots as PNG, so re-encode when we can.
        let (bytes, extension) = match ::image::load_from_memory(&shot.bytes) {
            Ok(decoded) => {
                let mut png = Vec::new();
                let cursor = std::io::Cursor::new(&mut png);
                match decoded.write_to(cursor, ::image::ImageFormat::Png) {
                    Ok(()) => (png, "png"),
                    Err(_) => (shot.bytes.clone(), shot.format.extension()),
                }
            }
            Err(_) => (shot.bytes.clone(), shot.format.extension()),
        };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("meowdown-{stamp}.{extension}"));
        if std::fs::write(&path, &bytes).is_err() {
            self.hint = "无法写出剪贴板里的图片".into();
            cx.notify();
            return true;
        }
        self.add_image(path, window, cx);
        true
    }

    // ------------------------------------------------------------- paragraph

    fn toggle_todo(&mut self, index: usize, _window: &mut Window, cx: &mut Context<Self>) {
        self.remember(self.blocks[index].id);
        let checked = matches!(self.blocks[index].kind, BlockKind::Todo(true));
        self.blocks[index].kind = BlockKind::Todo(!checked);
        self.dirty = true;
        cx.notify();
    }

    /// Turn the current paragraph into a display formula and back.
    fn toggle_math_block(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_index() else {
            self.hint = "先点进一个段落再插入公式".into();
            cx.notify();
            return;
        };
        let next = if self.blocks[index].kind.is_math() {
            BlockKind::Paragraph
        } else {
            BlockKind::Math
        };
        self.set_block_kind(index, next, window, cx);
    }

    fn toggle_code_block(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_index() else {
            self.hint = "先点进一个段落再插入代码块".into();
            cx.notify();
            return;
        };
        let next = if self.blocks[index].is_code() {
            BlockKind::Paragraph
        } else {
            BlockKind::Code(None)
        };
        self.set_block_kind(index, next, window, cx);
    }

    /// Ctrl+T: turn an empty paragraph into a table, otherwise put a fresh one
    /// below the block the caret is in. The template is the shape a hand-typed
    /// table starts with, so the caret can go straight into a cell.
    fn insert_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_index() else {
            self.hint = "先点进一个段落再插入表格".into();
            cx.notify();
            return;
        };
        let template = model::table_template();
        self.remember(self.blocks[index].id);
        let empty = self.block_text(index, cx).trim().is_empty()
            && !self.blocks[index].is_code()
            && !self.blocks[index].kind.is_table();
        if empty {
            self.blocks[index].kind = BlockKind::Table;
            self.blocks[index].text = template.clone().into();
            self.blocks[index]
                .state
                .update(cx, |state, cx| state.replace_all(template, window, cx));
            self.focus_block(index, window, cx);
        } else {
            let block = self.make_block(window, cx, PlainBlock::new(BlockKind::Table, template));
            self.blocks.insert(index + 1, block);
            self.focus_block(index + 1, window, cx);
        }
        self.dirty = true;
        self.schedule_autosave(window, cx);
        self.hint = "已插入表格 · 点单元格直接改".into();
        // Put the caret in the first cell, which is what "new table" means.
        let id = self.blocks[if empty { index } else { index + 1 }].id;
        self.edit_table_cell(id, None, 0, window, cx);
        cx.notify();
    }

    /// Open one table cell for typing. The grid stays visible: the field lives
    /// inside the cell, so the table is edited where it is read instead of
    /// somewhere under it in raw pipes and dashes.
    fn edit_table_cell(
        &mut self,
        block: u64,
        row: Option<usize>,
        column: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(block) else {
            return;
        };
        let Some(table) = model::parse_table(&self.blocks[index].text) else {
            return;
        };
        let seed = match row {
            None => table.header.get(column).cloned().unwrap_or_default(),
            Some(row) => table
                .rows
                .get(row)
                .and_then(|cells| cells.get(column))
                .cloned()
                .unwrap_or_default(),
        };
        // Already in this cell: only the focus needs to follow the click.
        if let Some(edit) = &self.table_edit {
            if edit.block == block && edit.row == row && edit.column == column {
                let state = edit.state.clone();
                state.update(cx, |state, cx| state.focus(window, cx));
                return;
            }
        }
        self.table_edit = None;
        self.active_id = Some(block);
        // One history step per cell, not per keystroke.
        self.remember(block);
        let seed_len = seed.len();
        // A single-line field, not a textarea: only a single-line state takes a
        // `text_align`, and a cell that is centred while it is read has to stay
        // centred while it is typed in. A Markdown cell never holds a line
        // break anyway — Enter moves to the next cell.
        let align = table
            .align
            .get(column)
            .copied()
            .unwrap_or(model::ColumnAlign::Left);
        let state = cx.new(|cx| {
            InputState::new(window, cx)
                .submit_on_enter(true)
                .default_value(seed)
        });
        state.update(cx, |state, cx| state.set_text_align(column_align(align), cx));
        let subscription = cx.subscribe_in(
            &state,
            window,
            move |this: &mut MeowDown,
                  _: &Entity<InputState>,
                  event: &InputEvent,
                  window: &mut Window,
                  cx: &mut Context<MeowDown>| {
                this.on_table_cell_event(block, row, column, event, window, cx)
            },
        );
        self.table_edit = Some(TableCellEdit {
            block,
            row,
            column,
            state: state.clone(),
            _subscription: subscription,
        });
        state.update(cx, |state, cx| {
            state.focus(window, cx);
            // Start at the end of the cell: a click opens it for editing the
            // text it already has, and appending is what that usually means.
            state.set_selected_range(seed_len..seed_len, cx);
        });
        cx.notify();
    }

    fn on_table_cell_event(
        &mut self,
        block: u64,
        row: Option<usize>,
        column: usize,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let Some(edit) = &self.table_edit else {
                    return;
                };
                let text = edit.state.read(cx).value().to_string();
                self.write_table_cell(block, row, column, text, window, cx);
                self.schedule_autosave(window, cx);
                cx.notify();
            }
            // Enter walks the caret through the table the way Tab does in a
            // spreadsheet, and leaves it when the last cell is done.
            InputEvent::PressEnter { .. } => {
                self.step_table_cell(block, row, column, window, cx);
            }
            InputEvent::Blur => {
                let current = self
                    .table_edit
                    .as_ref()
                    .is_some_and(|edit| edit.block == block && edit.row == row && edit.column == column);
                if current {
                    self.table_edit = None;
                    cx.notify();
                }
            }
            InputEvent::Focus => {}
        }
    }

    /// Write one cell back into the block's Markdown.
    fn write_table_cell(
        &mut self,
        block: u64,
        row: Option<usize>,
        column: usize,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(block) else {
            return;
        };
        let Some(mut table) = model::parse_table(&self.blocks[index].text) else {
            return;
        };
        let slot = match row {
            None => table.header.get_mut(column),
            Some(row) => table
                .rows
                .get_mut(row)
                .and_then(|cells| cells.get_mut(column)),
        };
        let Some(slot) = slot else {
            return;
        };
        // A bar would end the cell; the table serializer escapes it.
        let value = value.replace('\n', " ");
        if *slot == value {
            return;
        }
        *slot = value;
        let markdown = table.to_markdown();
        self.blocks[index].text = markdown.clone().into();
        // The plain field behind the block still holds the source, so the
        // source view (Ctrl+/) and the next save agree with the grid.
        let state = self.blocks[index].state.clone();
        state.update(cx, |state, cx| state.set_value(markdown, window, cx));
        self.dirty = true;
    }

    /// Move to the next cell, or step out of the table at its end.
    fn step_table_cell(
        &mut self,
        block: u64,
        row: Option<usize>,
        column: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(block) else {
            return;
        };
        let Some(table) = model::parse_table(&self.blocks[index].text) else {
            return;
        };
        let columns = table.columns().max(1);
        let next = match row {
            None => {
                if table.rows.is_empty() {
                    None
                } else {
                    Some((Some(0), 0))
                }
            }
            Some(row) if column + 1 < columns => Some((Some(row), column + 1)),
            Some(row) if row + 1 < table.rows.len() => Some((Some(row + 1), 0)),
            Some(_) => None,
        };
        match next {
            Some((row, column)) => self.edit_table_cell(block, row, column, window, cx),
            None => {
                self.table_edit = None;
                self.hint = "表格已写完 · 点任意单元格继续改".into();
                cx.notify();
            }
        }
    }

    /// Handle an Up/Down press: step into the neighbouring block when the caret
    /// already sits on the first or last line. `true` means the key was used.    ///
    /// Arrow keys arrive as `MoveUp`/`MoveDown` actions, which stop propagating
    /// on their own, so this has to run as a capture-phase action listener
    /// rather than from the window key-down handler.
    fn arrow_step(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.show_settings {
            return false;
        }
        let Some(index) = self.active_index() else {
            return false;
        };
        // A code editor owns its rows and scrolls on its own.
        if self.blocks[index].is_code() {
            return false;
        }
        // The language field is a separate one-line input.
        if self.lang_focus == Some(self.blocks[index].id) || self.lang_menu.is_some() {
            return false;
        }
        let single_line = !self.blocks[index].text.contains('\n');
        let step = if single_line && !self.block_wraps(index, cx) {
            true
        } else {
            self.caret_on_edge(cx, up)
        };
        if !step {
            return false;
        }
        self.step_block(if up { -1 } else { 1 }, up, window, cx);
        true
    }

    fn on_move_up(&mut self, _: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        // Up and Down walk the language list while it is open — the field
        // itself is one line long, so the keys would otherwise do nothing.
        if self.lang_menu.is_some() {
            self.lang_menu_step(-1, cx);
            cx.stop_propagation();
            return;
        }
        if self.arrow_step(true, window, cx) {
            cx.stop_propagation();
        }
    }

    fn on_move_down(&mut self, _: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        if self.lang_menu.is_some() {
            self.lang_menu_step(1, cx);
            cx.stop_propagation();
            return;
        }
        if self.arrow_step(false, window, cx) {
            cx.stop_propagation();
        }
    }

    /// Left and right are the viewer's while it is up, and the field's
    /// otherwise. They arrive as actions rather than key presses, so this is
    /// the only place they can be taken away from the paragraph behind.
    fn on_move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.viewer.is_some() {
            self.viewer_step(-1, cx);
            cx.stop_propagation();
        }
    }

    fn on_move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.viewer.is_some() {
            self.viewer_step(1, cx);
            cx.stop_propagation();
        }
    }

    /// Intercept Ctrl+V before the focused field swallows it, so a screenshot on
    /// the clipboard turns into an image block instead of being ignored.
    fn on_paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_settings || self.lang_focus.is_some() || self.viewer.is_some() {
            return;
        }
        if self.paste_clipboard_image(window, cx) {
            cx.stop_propagation();
        }
    }

    /// Ctrl+Z is bound to the field's own history by the keymap, which only
    /// knows about the paragraph that has focus. Intercepting the action in the
    /// capture phase keeps the document-level history in charge.
    fn on_undo(
        &mut self,
        _: &gpui_kit::component::input::Undo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Source mode edits one big text field, whose own history is right.
        if self.show_settings || self.source_mode {
            return;
        }
        self.undo(window, cx);
        cx.stop_propagation();
    }

    fn on_redo(
        &mut self,
        _: &gpui_kit::component::input::Redo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.show_settings || self.source_mode {
            return;
        }
        self.redo(window, cx);
        cx.stop_propagation();
    }

    /// Move the caret to the previous or next block when the arrow key runs out
    /// of lines, like Typora does.
    fn step_block(&mut self, delta: i8, to_end: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_index() else {
            return;
        };
        let target = index as isize + delta as isize;
        if target < 0 || target as usize >= self.blocks.len() {
            return;
        }
        let target = target as usize;
        self.focus_block(target, window, cx);
        let length = self.blocks[target].state.read(cx).value().len();
        let caret = if to_end { length } else { 0 };
        let state = self.blocks[target].state.clone();
        state.update(cx, |state, cx| state.set_selected_range(caret..caret, cx));
        cx.notify();
    }

    fn caret_on_edge(&self, cx: &App, at_start: bool) -> bool {
        let Some(index) = self.active_index() else {
            return false;
        };
        let range = self.blocks[index].state.read(cx).selected_range();
        if !range.is_empty() {
            return false;
        }
        let length = self.blocks[index].state.read(cx).value().len();
        if at_start {
            range.start == 0
        } else {
            range.end >= length
        }
    }

    /// Whether a block is tall enough that its text must have wrapped.
    fn block_wraps(&self, index: usize, cx: &App) -> bool {
        let id = self.blocks[index].id;
        let Some(bounds) = self.bounds.borrow().get(&id).copied() else {
            return false;
        };
        let (size, _, _) = block_type_style(&self.blocks[index].kind, &cx.theme().colors);
        bounds.size.height > px(size * LINE_RATIO * 1.4)
    }

}

/// What the renderer can do with a web image right now.
enum RemoteState {
    Ready(Arc<Image>),
    Pending,
    Failed,
}

fn is_remote(src: &str) -> bool {
    let lower = src.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("data:")
}

/// The width and height a picture was encoded at, read from its bytes without
/// decoding the pixels — a header read, not a full decode.
fn image_size_from_bytes(bytes: &[u8]) -> Option<(u32, u32)> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.into_dimensions().ok()
}

/// Where the next picture is, wrapping at both ends.
fn wrapped_index(at: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = len as i32;
    (((at as i32 + delta) % len) + len) as usize % len as usize
}

/// How much a picture is scaled by to fit the window: it shrinks to fit, and a
/// picture already smaller than the window is left at its own size — blowing a
/// 64px icon up to fill the screen only makes it blurry.
fn viewer_fit(image: Size<f32>, viewport: Size<Pixels>) -> f32 {
    let room_w = (viewport.width.as_f32() - VIEWER_PAD * 2.0).max(1.0);
    let room_h = (viewport.height.as_f32() - VIEWER_PAD * 2.0).max(1.0);
    let fit = (room_w / image.width).min(room_h / image.height);
    fit.min(1.0).max(0.01)
}

/// The rectangle the picture occupies at a given zoom and offset.
fn viewer_bounds(
    image: Size<f32>,
    viewport: Size<Pixels>,
    zoom: f32,
    offset: Point<Pixels>,
) -> Bounds<Pixels> {
    let scale = viewer_fit(image, viewport) * zoom;
    let width = image.width * scale;
    let height = image.height * scale;
    let left = (viewport.width.as_f32() - width) / 2.0 + offset.x.as_f32();
    let top = (viewport.height.as_f32() - height) / 2.0 + offset.y.as_f32();
    Bounds::new(point(px(left), px(top)), size(px(width), px(height)))
}

/// The offset that keeps the point under `anchor` where it is while the
/// picture grows by `factor`. Everything is measured from the middle of the
/// window, which is where the picture is centred when the offset is zero.
fn zoomed_offset(
    offset: Point<Pixels>,
    anchor: Point<Pixels>,
    center: Point<Pixels>,
    factor: f32,
) -> Point<Pixels> {
    let rel = point(
        anchor.x - center.x - offset.x,
        anchor.y - center.y - offset.y,
    );
    point(
        offset.x - rel.x * (factor - 1.0),
        offset.y - rel.y * (factor - 1.0),
    )
}

fn is_divider(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.len() >= 3
        && (trimmed.chars().all(|c| c == '-')
            || trimmed.chars().all(|c| c == '*')
            || trimmed.chars().all(|c| c == '_'))
}

fn detect_shortcut(text: &str) -> Option<(BlockKind, String)> {
    for level in (1..=6u8).rev() {
        let prefix = "#".repeat(level as usize) + " ";
        if let Some(rest) = text.strip_prefix(prefix.as_str()) {
            return Some((BlockKind::Heading(level), rest.to_string()));
        }
    }

    for marker in ["- [ ] ", "- [x] ", "- [X] ", "* [ ] ", "* [x] "] {
        if let Some(rest) = text.strip_prefix(marker) {
            let checked = marker.contains("[x]") || marker.contains("[X]");
            return Some((BlockKind::Todo(checked), rest.to_string()));
        }
    }

    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = text.strip_prefix(marker) {
            return Some((BlockKind::Bullet, rest.to_string()));
        }
    }

    if let Some(rest) = text.strip_prefix("> ") {
        return Some((BlockKind::Quote, rest.to_string()));
    }

    // `$$` on its own line turns the paragraph into a display formula.
    if let Some(rest) = text.strip_prefix("$$") {
        if let Some(body) = rest.strip_suffix("$$") {
            return Some((BlockKind::Math, body.trim().to_string()));
        }
        if rest.trim().is_empty() {
            return Some((BlockKind::Math, String::new()));
        }
    }

    let digits = text.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let rest = &text[digits..];
        if let Some(rest) = rest.strip_prefix(". ") {
            return Some((BlockKind::Ordered, rest.to_string()));
        }
    }

    None
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// Extensions counted as Markdown in the folder listing.
fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("md" | "markdown" | "mdown" | "mkd")
    )
}

/// Whether a path is a file this window can open as prose. A `.txt` is as
/// editable as a `.md`; a `.png` next to it is not.
fn is_plain_text(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("txt" | "text")
    )
}

/// Whether an address names a scheme — a place the desktop has to open, not a
/// file next to the document.
fn has_scheme(url: &str) -> bool {
    let lowered = url.to_ascii_lowercase();
    ["http://", "https://", "mailto:", "file://", "ftp://", "tel:"]
        .iter()
        .any(|scheme| lowered.starts_with(scheme))
}

/// The comparable form of a heading or of the fragment that points at one.
///
/// `## My heading` is addressed as `#my-heading`, `## 一段标题` as `#一段标题`,
/// and a real link carries both percent-encoded. Dropping everything but
/// letters and digits — after decoding, and after lowercasing — leaves the same
/// string for `my-heading`, `My Heading` and `%E4%B8%80%E6%AE%B5%E6%A0%87%E9%A2%98`.
fn anchor_key(text: &str) -> String {
    percent_decode(text)
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

/// `%E4%B8%80` → `一`. Anything that is not a complete escape is left alone,
/// so a stray `%` in a heading does not eat the character after it.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let digit = |byte: u8| (byte as char).to_digit(16);
            if let (Some(high), Some(low)) = (digit(bytes[index + 1]), digit(bytes[index + 2])) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether a directory entry names a folder, and whether it names a file.
///
/// Asked of the entry rather than of its path: `is_dir`/`is_file` cost a `stat`
/// per name — around 130 ms for six thousand notes in one folder, against 2 ms
/// for the type that comes with the listing. A link is the one case that still
/// has to be followed.
fn entry_flags(entry: &std::fs::DirEntry) -> (bool, bool) {
    match entry.file_type() {
        Ok(kind) if kind.is_symlink() => {
            let path = entry.path();
            (path.is_dir(), path.is_file())
        }
        Ok(kind) => (kind.is_dir(), kind.is_file()),
        Err(_) => (false, false),
    }
}

/// Every Markdown file directly inside `dir`, sorted by name.
fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry_flags(entry).1)
        .map(|entry| entry.path())
        .filter(|path| is_markdown(path))
        .collect();
    files.sort_by(|a, b| {
        file_name(a)
            .to_lowercase()
            .cmp(&file_name(b).to_lowercase())
            .then_with(|| file_name(a).cmp(&file_name(b)))
    });
    files
}

/// Folders a walk never descends into: nothing a writer keeps there.
fn skip_dir(path: &Path) -> bool {
    let name = file_name(path);
    name.starts_with('.')
        || matches!(
            name.as_str(),
            "node_modules" | "target" | "dist" | "build" | "vendor"
        )
}

/// One directory in the side panel's tree: the folders under it, and the
/// Markdown files directly inside it.
///
/// A node is a *stub* until someone opens it: it knows its name and path, but
/// `dirs` and `files` stay empty until [`Self::load`] reads the directory. A
/// folder holding a thousand notes across a dozen levels used to be walked in
/// full before the first frame was drawn — and then walked again on every
/// rescan, which is what made a busy parent folder feel like a hung program.
/// Now each level is read when it is shown, and not before.
struct TreeNode {
    name: String,
    path: PathBuf,
    dirs: Vec<TreeNode>,
    files: Vec<PathBuf>,
    /// Whether `dirs` and `files` came from the disk. An unread node may well
    /// hold something, so it is never drawn as empty.
    loaded: bool,
}

impl TreeNode {
    fn stub(path: &Path) -> TreeNode {
        TreeNode {
            name: folder_name(path),
            path: path.to_path_buf(),
            dirs: Vec::new(),
            files: Vec::new(),
            loaded: false,
        }
    }

    /// Read this level from the disk, once.
    ///
    /// `loaded` is set before the read: a folder that cannot be read stays an
    /// empty level rather than being retried on every frame.
    fn load(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let Ok(entries) = std::fs::read_dir(&self.path) else {
            return;
        };
        // Every folder inside is kept, even one that holds no Markdown at all:
        // a folder the user made should not go missing from the list just
        // because it is empty or only holds images.
        let mut dirs: Vec<PathBuf> = Vec::new();
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            let (is_dir, is_file) = entry_flags(&entry);
            if is_dir {
                if !skip_dir(&path) {
                    dirs.push(path);
                }
            } else if is_file && is_markdown(&path) {
                self.files.push(path);
            }
        }
        self.files
            .sort_by_key(|path| file_name(path).to_lowercase());
        dirs.sort_by_key(|path| folder_name(path).to_lowercase());
        self.dirs = dirs.iter().map(|path| TreeNode::stub(path)).collect();
    }

    /// Read every level between here and `path`, so a branch that is open has
    /// something in it again after the tree has been rebuilt.
    fn load_path(&mut self, path: &Path) {
        self.load();
        for child in &mut self.dirs {
            if path.starts_with(&child.path) {
                child.load_path(path);
                break;
            }
        }
    }

    /// Markdown files in this folder alone. `None` until the folder has been
    /// read: not knowing is the whole point of the lazy tree.
    fn file_count(&self) -> Option<usize> {
        self.loaded.then_some(self.files.len())
    }

    /// A folder known to have nothing to show: no Markdown of its own and no
    /// subfolder either. It stays in the list — dimmed and unopenable — so the
    /// tree keeps matching what the disk actually has. A folder that has not
    /// been read is never called empty.
    fn shows_nothing(&self) -> bool {
        self.loaded && self.dirs.is_empty() && self.files.is_empty()
    }
}

/// The tree for `root`, with its top level already read: the panel always
/// shows that much.
fn folder_tree(root: &Path) -> TreeNode {
    let mut node = TreeNode::stub(root);
    node.load();
    node
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// A stable, allocation-free id for a row's path.
///
/// `ElementId::Name(format!("file:{}", path.display()))` is the obvious spelling
/// and it was the one used here; with a few thousand notes in a folder it also
/// meant a few thousand formatted strings on every frame, which is lag you can
/// feel while typing.
fn path_id(path: &Path) -> ElementId {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    ElementId::Integer(hasher.finish())
}

/// Path of `path` relative to `root`, with forward slashes so it reads the same
/// on every platform.
fn relative_label(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// ------------------------------------------------------------ file tree pieces

/// Height of one entry in the file menu.
const MENU_ROW: f32 = 27.;
/// Width of the file menu panel.
const MENU_WIDTH: f32 = 208.;

// -------------------------------------------------------- language drop-down

/// Width of the panel that lists the languages a fence can be written in.
const LANG_PANEL_WIDTH: f32 = 224.;
/// Height of one language in that panel.
const LANG_ROW: f32 = 25.;
/// How many languages are in sight before the list starts to scroll. Eight is
/// about the height of a code block's own corner, so the panel covers the code
/// it belongs to and no more.
const LANG_ROWS_SHOWN: usize = 8;

/// Every language a fence can be written in, in the order the list shows them.
///
/// This is the very set the code editor highlights with, so nothing is offered
/// that the fence could not then colour. It is built on the first open rather
/// than at startup: the dump holds a couple of hundred syntaxes and unpacking
/// it costs more than drawing the window.
fn all_languages() -> &'static [LangChoice] {
    static ALL: OnceLock<Vec<LangChoice>> = OnceLock::new();
    ALL.get_or_init(|| {
        let syntaxes = two_face::syntax::extra_newlines();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut list: Vec<LangChoice> = Vec::new();
        for syntax in syntaxes.syntaxes() {
            // The plain-text fallback is the absence of a language, not one of
            // the choices.
            if syntax.name == "Plain Text" {
                continue;
            }
            // The first extension is the canonical token: `rs`, `py`, `cpp`.
            // A syntax without one cannot be named in a fence at all.
            let Some(token) = syntax.file_extensions.first().filter(|ext| !ext.is_empty()) else {
                continue;
            };
            if !seen.insert(token.clone()) {
                continue;
            }
            list.push(LangChoice {
                token: token.clone(),
                name: syntax.name.clone(),
            });
        }
        list.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
        list
    })
}

/// The rows that match what has been typed into the field.
///
/// Matching is what matters, ordering is the polish: a row that starts with what
/// was typed comes before one that merely contains it, and a fence token before
/// a display name — so "c" offers C, C++ and C# up front, and `Arduino C`
/// further down, instead of burying the short names under the long ones.
fn lang_matches(query: &str) -> Vec<LangChoice> {
    let query = query.trim().to_ascii_lowercase();
    let all = all_languages();
    if query.is_empty() {
        return all.to_vec();
    }
    let mut scored: Vec<(u8, LangChoice)> = all
        .iter()
        .filter_map(|choice| {
            let token = choice.token.to_ascii_lowercase();
            let name = choice.name.to_ascii_lowercase();
            let rank = if token == query {
                0
            } else if token.starts_with(&query) {
                1
            } else if name.starts_with(&query) {
                2
            } else if token.contains(&query) {
                3
            } else if name.contains(&query) {
                4
            } else {
                return None;
            };
            Some((rank, choice.clone()))
        })
        .collect();
    // The sort is stable, so rows of equal rank keep the list's own (name)
    // order instead of shuffling on every keystroke.
    scored.sort_by_key(|(rank, _)| *rank);
    scored.into_iter().map(|(_, choice)| choice).collect()
}

/// One command in the file menu. The listener closes the menu first, so the
/// panel is gone before whatever it opened is drawn on top.
fn menu_item<F>(
    label: &'static str,
    theme: &ThemeColor,
    cx: &mut Context<MeowDown>,
    run: F,
) -> AnyElement
where
    F: Fn(&mut MeowDown, &mut Window, &mut Context<MeowDown>) + 'static,
{
    div()
        .id(SharedString::from(format!("file-menu-{label}")))
        .flex()
        .items_center()
        .w_full()
        .h(px(MENU_ROW))
        .mx(px(4.))
        .px_2()
        .rounded(px(5.))
        .text_sm()
        .text_color(theme.foreground)
        .cursor_pointer()
        .hover(|style| style.bg(theme.sidebar_accent))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                this.menu = None;
                run(this, window, cx);
            }),
        )
        .child(label)
        .into_any_element()
}

/// The hairline between groups of commands.
fn menu_rule(theme: &ThemeColor) -> AnyElement {
    div()
        .mx_2()
        .my(px(3.))
        .h(px(1.))
        .bg(theme.border)
        .into_any_element()
}

/// One button of a prompt's footer.
fn prompt_button<F>(
    label: &'static str,
    primary: bool,
    theme: &ThemeColor,
    cx: &mut Context<MeowDown>,
    run: F,
) -> AnyElement
where
    F: Fn(&mut MeowDown, &mut Window, &mut Context<MeowDown>) + 'static,
{
    div()
        .id(SharedString::from(format!("prompt-{label}")))
        .flex()
        .items_center()
        .justify_center()
        .h(px(28.))
        .px(px(14.))
        .rounded(px(6.))
        .text_sm()
        .cursor_pointer()
        .when(primary, |this| {
            this.bg(theme.primary)
                .text_color(theme.primary_foreground)
                .hover(|style| style.opacity(0.88))
        })
        .when(!primary, |this| {
            this.bg(theme.muted)
                .text_color(theme.foreground)
                .hover(|style| style.bg(theme.secondary))
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| run(this, window, cx)),
        )
        .child(label)
        .into_any_element()
}

/// A name nobody has taken yet: `新建文档.md`, then `新建文档 2.md`, and so on.
fn unused_path(dir: &Path, stem: &str, ext: Option<&str>) -> PathBuf {
    let build = |name: &str| match ext {
        Some(ext) => dir.join(format!("{name}.{ext}")),
        None => dir.join(name),
    };
    let first = build(stem);
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let candidate = build(&format!("{stem} {n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

/// Where a duplicate of `path` goes: `笔记 - 副本.md`, then `笔记 - 副本 2.md`.
fn copy_path_for(path: &Path) -> PathBuf {
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name(path));
    let ext = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_string());
    let build = |suffix: &str| match &ext {
        Some(ext) => dir.join(format!("{stem}{suffix}.{ext}")),
        None => dir.join(format!("{stem}{suffix}")),
    };
    let first = build(" - 副本");
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let candidate = build(&format!(" - 副本 {n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

/// Why `name` cannot be a file name on Windows, or `None` when it can.
fn name_problem(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("名字不能为空".to_string());
    }
    if let Some(bad) = name
        .chars()
        .find(|ch| "\\/:*?\"<>|".contains(*ch))
    {
        return Some(format!("名字里不能有 {bad}"));
    }
    if name == "." || name == ".." {
        return Some("这个名字不能用".to_string());
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Some("名字不能以点或空格结尾".to_string());
    }
    None
}

/// What the properties card calls a file.
fn describe_kind(path: &Path) -> String {
    let Some(ext) = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
    else {
        return "文件".to_string();
    };
    if is_markdown(path) {
        format!("Markdown 文档（.{ext}）")
    } else {
        format!("{} 文件", ext.to_ascii_uppercase())
    }
}

/// How much sits directly inside a folder: (files, folders).
fn count_children(dir: &Path) -> (usize, usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut files = 0;
    let mut folders = 0;
    for entry in entries.filter_map(|entry| entry.ok()) {
        if entry_flags(&entry).0 {
            folders += 1;
        } else {
            files += 1;
        }
    }
    (files, folders)
}

/// Width of the reading column for a window `viewport` wide, with a side panel
/// of `sidebar` pixels taking its share.
///
/// The prose takes [`PAGE_WIDTH_RATIO`] of what is left and is centred in it,
/// so the margins grow with the window exactly as the text does. On a window
/// too narrow even for the minimum measure, the margins are what goes.
fn reading_column(viewport: Pixels, sidebar: f32) -> Pixels {
    let available = (viewport - px(sidebar)).max(px(PAGE_MIN_WIDTH));
    (available * PAGE_WIDTH_RATIO)
        .max(px(PAGE_MIN_WIDTH))
        .min(available)
}

/// Markdown files under `root` whose path inside the folder holds `query`.
///
/// Matching the relative path (not just the file name) means typing a folder
/// name lists what is in it — the usual way a search box is used here.
/// Bounded on purpose: the box is for finding a note in the folder on screen,
/// not for indexing a disk, so a runaway match count stops at [`SEARCH_LIMIT`].
fn search_hits(root: &Path, query: &str) -> Vec<PathBuf> {
    const SEARCH_LIMIT: usize = 200;
    const MAX_DEPTH: usize = 12;
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<PathBuf> = Vec::new();
    let mut queue: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            let (is_dir, is_file) = entry_flags(&entry);
            if is_dir {
                if depth < MAX_DEPTH && !skip_dir(&path) {
                    queue.push((path, depth + 1));
                }
            } else if is_file
                && is_markdown(&path)
                && relative_label(root, &path).to_lowercase().contains(&needle)
            {
                hits.push(path);
                if hits.len() >= SEARCH_LIMIT {
                    break;
                }
            }
        }
        if hits.len() >= SEARCH_LIMIT {
            break;
        }
    }
    hits.sort_by_key(|path| relative_label(root, path).to_lowercase());
    hits
}

/// Font size, weight and color used while a block is being edited.
fn block_type_style(kind: &BlockKind, theme: &ThemeColor) -> (f32, bool, Hsla) {
    match kind {
        BlockKind::Heading(1) => (32.0, true, theme.foreground),
        BlockKind::Heading(2) => (26.0, true, theme.foreground),
        BlockKind::Heading(3) => (21.0, true, theme.foreground),
        BlockKind::Heading(4) => (18.0, true, theme.foreground),
        BlockKind::Heading(_) => (16.5, true, theme.foreground),
        BlockKind::Quote => (16.5, false, theme.muted_foreground),
        _ => (16.5, false, theme.foreground),
    }
}

/// Extra space above a block, so headings breathe like they do in Typora.
fn block_space_above(kind: &BlockKind) -> f32 {
    match kind {
        BlockKind::Heading(1) => 20.0,
        BlockKind::Heading(2) => 16.0,
        BlockKind::Heading(3) => 12.0,
        BlockKind::Heading(_) => 8.0,
        BlockKind::Rule => 6.0,
        _ => 0.0,
    }
}

/// A fence opener: ```` ``` ```` or ```` ```rust ````, with nothing else on the
/// line. `None` means the line is not a fence at all.
fn fence_language(line: &str) -> Option<Option<String>> {
    let rest = line.trim().strip_prefix("```")?;
    let language = rest.trim();
    if language.split_whitespace().count() > 1 {
        return None;
    }
    Some(if language.is_empty() {
        None
    } else {
        Some(language.to_string())
    })
}

/// Map an x offset inside one line of text to a byte index, so a click lands
/// between the letters it was aimed at.
/// Rows a piece of code occupies once laid out at `width`, soft wraps included.
///
/// The editor's own `auto_grow` mode is only exposed on text areas, so the
/// editor keeps a definite height and this is what keeps it honest: a long
/// line that wraps to three rows counts as three, not one.
fn wrapped_rows(text: &str, width: Pixels, window: &mut Window) -> usize {
    // A trailing newline puts the caret on an extra row that the shaper drops.
    let logical = text.lines().count().max(1) + usize::from(text.ends_with('\n'));
    if text.is_empty() || width <= px(0.) {
        return logical;
    }
    let run = TextRun {
        len: text.len(),
        font: Font {
            family: mono_font().into(),
            ..Default::default()
        },
        color: black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    // `shape_text` hands back one entry per hard line and records where each
    // of them had to wrap, so the visual rows are the wraps plus one a line.
    let shaped = window.text_system().shape_text(
        SharedString::from(text.to_string()),
        px(CODE_TEXT_SIZE),
        &[run],
        Some(width),
        None,
    );
    match shaped {
        Ok(lines) => lines
            .iter()
            .map(|line| line.wrap_boundaries().len() + 1)
            .sum::<usize>()
            .max(logical),
        Err(_) => logical,
    }
}

/// Where two versions of the same prose start and stop differing.
///
/// Returns the byte index the change starts at, the range it replaced in the
/// old text, and the text that took its place. A keystroke, a paste and a
/// deletion are all one of these.
fn text_diff<'a>(old: &str, new: &'a str) -> (usize, Range<usize>, &'a str) {
    let mut start = 0;
    let mut limit = old.len().min(new.len());
    while start < limit && old.as_bytes()[start] == new.as_bytes()[start] {
        start += 1;
    }
    while start > 0 && (!old.is_char_boundary(start) || !new.is_char_boundary(start)) {
        start -= 1;
    }
    let mut tail = 0;
    limit = limit.saturating_sub(start);
    while tail < limit
        && old.as_bytes()[old.len() - 1 - tail] == new.as_bytes()[new.len() - 1 - tail]
    {
        tail += 1;
    }
    while tail > 0
        && (!old.is_char_boundary(old.len() - tail) || !new.is_char_boundary(new.len() - tail))
    {
        tail -= 1;
    }
    (
        start,
        start..old.len() - tail,
        &new[start..new.len() - tail],
    )
}

/// The prose a paragraph shows for a caret at `caret` — a *source* offset.
///
/// Everything is rendered with its markers hidden; the one run the caret sits
/// inside is the exception, so the syntax appears exactly where the editing
/// happens and nowhere else on the line.
fn paragraph_view(text: &str, caret: Option<usize>) -> inline::Inline {
    let base = inline::render(text);
    match caret.and_then(|at| base.opened_at(at, text)) {
        Some(view) => view,
        None => base,
    }
}

/// The prose for `caret`, together with the place that caret has in it.
///
/// Two maps meet here and they are not exact inverses at the edge of a run: a
/// pointer just past `**bold**` resolves to the byte *after* the markers, while
/// that same display position maps back to the last character of `bold`. Left
/// alone, the pair disagrees, and the next frame — which reads the caret back
/// out of the field exactly this way — decides the run under it has changed.
/// The prose is then rebuilt, the caret lands somewhere else, and the round
/// trip starts over: the markers flicker and typing goes to the wrong place.
///
/// So the two are settled here, by asking each map about the other's answer
/// until neither has anything left to say. It converges in one step for
/// everything but the seam of a run, and in two at the seam.
fn view_at(text: &str, caret: usize) -> (inline::Inline, usize) {
    let mut at = caret;
    let mut view = paragraph_view(text, Some(at));
    let mut placed = view.display_of(at);
    for _ in 0..4 {
        let back = view.source_point(placed);
        if back == at {
            break;
        }
        at = back;
        view = paragraph_view(text, Some(at));
        placed = view.display_of(at);
    }
    (view, placed)
}

/// How many visual rows a paragraph of `size` type takes at `width`.
///
/// The field's own wrap map is only filled in once it has been laid out, which
/// is a frame later than the box that has to be sized for it — so the same
/// shape the pointer hit-testing uses answers the question up front.
fn prose_rows(
    text: &str,
    size: f32,
    bold: bool,
    chips: &[Range<usize>],
    width: Pixels,
    window: &mut Window,
) -> usize {
    let logical = text.lines().count().max(1);
    if text.is_empty() || width <= px(8.) {
        return logical;
    }
    // Measured with the same faces the line is drawn in — a chip is a run of
    // the code face, and a different face wraps at a different word.
    let runs = line_runs(text, bold, chips);
    match window.text_system().shape_text(
        SharedString::from(text.to_string()),
        px(size),
        &runs,
        Some(width),
        None,
    ) {
        Ok(lines) => lines
            .iter()
            .map(|line| line.wrap_boundaries().len() + 1)
            .sum::<usize>()
            .max(logical),
        Err(_) => logical,
    }
}

/// The byte of `text` under a point inside a paragraph of `size` type laid out
/// `width` wide, plus the width of the visual row the point fell on.
///
/// A paragraph wraps, so the point has to be turned into "which visual row"
/// first and only then into a column: a click on the second row of a long
/// paragraph is still the same logical line, and asking for its column straight
/// away drops the caret at the end of the first row. The wrap points come from
/// the very shaper the renderer uses, so the rows here are the rows on screen.
///
/// The row width comes back as well because "the pointer is past the last
/// glyph" is a different request from "the pointer is between these two
/// characters": the first means *end of what I can see*.
fn caret_at_point(
    text: &str,
    size: f32,
    bold: bool,
    chips: &[Range<usize>],
    width: Pixels,
    x: Pixels,
    y: Pixels,
    window: &mut Window,
) -> (usize, f32) {
    if text.is_empty() {
        return (0, 0.0);
    }
    // The same runs the paragraph was laid out with: a chip's content is set in
    // the code face, which is narrower or wider than the prose face, so a line
    // measured with one run would answer with the wrong character anywhere
    // from a chip onwards.
    let runs = line_runs(text, bold, chips);
    let Ok(lines) = window.text_system().shape_text(
        SharedString::from(text.to_string()),
        px(size),
        &runs,
        Some(width.max(px(8.))),
        None,
    ) else {
        return (0, 0.0);
    };

    let line_height = size * LINE_RATIO;
    let target = if line_height > 0.0 {
        (y.as_f32() / line_height).floor().max(0.0) as usize
    } else {
        0
    };
    // Byte offset of the current logical line inside `text`.
    let mut base = 0usize;
    // Visual row of the first row of the current logical line.
    let mut row = 0usize;
    let last = lines.len().saturating_sub(1);

    for (line_index, line) in lines.iter().enumerate() {
        let layout = &line.unwrapped_layout;
        // Every visual row of this line: (byte within the line, x within it).
        let mut rows: Vec<(usize, f32)> = vec![(0, 0.0)];
        for boundary in line.wrap_boundaries() {
            let glyph = layout
                .runs
                .get(boundary.run_ix)
                .and_then(|run| run.glyphs.get(boundary.glyph_ix));
            if let Some(glyph) = glyph {
                rows.push((glyph.index, glyph.position.x.as_f32()));
            }
        }
        let rows_here = rows.len();
        if target < row + rows_here || line_index == last {
            let local = target.saturating_sub(row).min(rows_here - 1);
            let (row_start, row_start_x) = rows[local];
            // Where this row stops. The next row starts there; the last row of
            // the line ends where the unwrapped layout ends.
            let row_end = rows
                .get(local + 1)
                .map(|(_, next_x)| *next_x)
                .unwrap_or(layout.width.as_f32());
            // `closest_index_for_x` speaks the coordinates of the *unwrapped*
            // line, so the row's own offset is added back.
            let index = layout.closest_index_for_x(x + px(row_start_x));
            let index = index.max(row_start).min(layout.len);
            return (boundary(text, base + index), (row_end - row_start_x).max(0.0));
        }
        row += rows_here;
        if line_index < last {
            base += layout.len + 1;
        }
    }
    (text.len(), 0.0)
}

/// Walk a position in the rendered prose back to the Markdown source.
///
/// `gaps` says which source range each inline formula replaced, because those
/// are blanks whose count depends on how wide the formula was typeset. A click
/// inside one of them belongs at the start of the formula it stands for.
fn display_to_source(
    inline: &inline::Inline,
    gaps: &[(Range<usize>, Range<usize>)],
    at: usize,
) -> usize {
    let mut delta: isize = 0;
    for (old, new) in gaps {
        if at >= new.end {
            delta += new.len() as isize - old.len() as isize;
        } else if at >= new.start {
            return inline.source_at(old.start);
        } else {
            break;
        }
    }
    let index = (at as isize - delta).max(0) as usize;
    inline.source_at(index.min(inline.text.len()))
}

/// The soft shadow a picture casts on the page.
///
/// One wide, low-alpha shadow rather than a stack of hairlines: several layers
/// close together read as a border, which fights the rounded corners and makes
/// a screenshot look framed. On paper the shadow is a hint of grey; on a dark
/// page black has to be darker still or it disappears into the background.
fn picture_shadow(theme: &ThemeColor) -> Vec<BoxShadow> {
    let alpha = if theme.background.l < 0.5 { 0.55 } else { 0.13 };
    vec![
        BoxShadow::new(px(0.), px(2.), hsla(0., 0., 0., alpha))
            .blur_radius(px(10.))
            .spread_radius(px(-2.)),
    ]
}

/// The blue a link is written in, picked from how dark the page is.
fn link_color(theme: &ThemeColor) -> Hsla {
    if theme.background.l < 0.5 {
        // Dark page: a light, slightly desaturated blue reads well on black and
        // does not glare like pure `#4d8cff` would.
        Hsla::from(rgb(0x7a_a7_f5))
    } else {
        // Paper: the usual link blue, dark enough to hold contrast on white.
        Hsla::from(rgb(0x1b_5f_c9))
    }
}

/// The fill behind a code block.
///
/// Pure white makes the card read as part of the paper, so the grey is a step
/// off the page — `#fafafa` on light themes, a raised charcoal on dark ones.
fn code_fill(theme: &ThemeColor) -> Hsla {
    if theme.background.l < 0.5 {
        Hsla::from(rgb(0x26_2a_31))
    } else {
        Hsla::from(rgb(0xfa_fa_fa))
    }
}

/// The tint behind inline `code`.
///
/// A cool grey, a shade off the page: enough to read as a chip without
/// colouring the sentence. Blue was tried first and pulled the eye away from
/// the prose — inline code is a texture, not an accent.
fn code_chip(theme: &ThemeColor) -> Hsla {
    if theme.background.l < 0.5 {
        Hsla::from(rgb(0x2f_34_3b))
    } else {
        Hsla::from(rgb(0xf3_f4_f4))
    }
}

/// The hairline around an inline code chip.
///
/// The fill alone is a shade off the page, which at a glance reads as a smudge
/// rather than an edge — the chip needs a border to have a shape at all. One
/// step darker than the fill, never a line the eye stops at.
fn code_chip_edge(theme: &ThemeColor) -> Hsla {
    if theme.background.l < 0.5 {
        Hsla::from(rgb(0x44_4b_55))
    } else {
        Hsla::from(rgb(0xe4_e6_e7))
    }
}

/// Paint one chip: the round fill the paragraph draws behind its code runs.
fn paint_chip(window: &mut Window, bounds: Bounds<Pixels>, theme: &ThemeColor) {
    window.paint_quad(
        fill(bounds, code_chip(theme))
            .corner_radii(px(CHIP_RADIUS))
            .border_widths(px(1.))
            .border_color(code_chip_edge(theme)),
    );
}

/// A table column's alignment, in the text engine's terms. The static cell
/// places its text with `justify_*`; the field that opens on top of it has to
/// be told the same thing separately, or the text jumps to the left the moment
/// the cell is clicked.
fn column_align(align: model::ColumnAlign) -> TextAlign {
    match align {
        model::ColumnAlign::Left => TextAlign::Left,
        model::ColumnAlign::Center => TextAlign::Center,
        model::ColumnAlign::Right => TextAlign::Right,
    }
}

/// Turn an inline style mask into the attributes the text renderer applies.
fn inline_style(flags: u8, theme: &ThemeColor) -> HighlightStyle {
    let code = code_chip(theme);
    HighlightStyle {
        // A link has to read as a link at a glance. The theme's own `link`
        // colour is the body colour in this palette (black on white, white on
        // black), so the blue is chosen here instead — a deep one on paper, a
        // light one on a dark page.
        color: if flags & inline::LINK != 0 {
            Some(link_color(theme))
        } else {
            None
        },
        font_weight: if flags & inline::BOLD != 0 {
            Some(FontWeight::BOLD)
        } else if flags & inline::CODE != 0 {
            // A row can only be set in one size, so inline code cannot be
            // smaller than the prose around it. The chip does that work
            // instead: a tinted, padded pill already reads as a quieter thing
            // than the sentence holding it. The glyphs keep the code face's
            // regular weight — what IDEA sets code in, and the weight that
            // stays level with the Chinese characters beside it. Light was
            // tried and comes out spindly at fifteen pixels on grey.
            Some(FontWeight::NORMAL)
        } else {
            None
        },
        font_style: if flags & inline::ITALIC != 0 {
            Some(FontStyle::Italic)
        } else {
            None
        },
        // Inline code gets a rounded chip painted underneath (see
        // `code_chips`), so the run itself only carries the highlight here.
        background_color: if flags & inline::MARK != 0 {
            Some(code)
        } else {
            None
        },
        underline: if flags & (inline::LINK | inline::UNDERLINE) != 0 {
            Some(UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: false,
            })
        } else {
            None
        },
        strikethrough: if flags & inline::STRIKE != 0 {
            Some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            })
        } else {
            None
        },
        fade_out: None,
    }
}

/// An element that draws a formula at exactly the size it was laid out at.
///
/// The box is built while rendering (it needs the text system) and then shared
/// with the paint pass through an `Rc`, because the canvas callbacks are `Fn`
/// and a formula is not `Clone`.
fn math_canvas(math: math::MathBox, width: f32, height: f32) -> AnyElement {
    let math = Rc::new(math);
    let prepaint = {
        let math = math.clone();
        move |_: Bounds<Pixels>, _: &mut Window, _: &mut App| math.clone()
    };
    canvas(
        prepaint,
        |bounds: Bounds<Pixels>, math: Rc<math::MathBox>, window: &mut Window, cx: &mut App| {
            math.paint(bounds.origin, window, cx);
        },
    )
    .w(px(width))
    .h(px(height))
    .flex_shrink_0()
    .into_any_element()
}

/// Replace inline `$…$` maths with blank space as wide as the typeset formula.
///
/// The paragraph is still laid out by the text engine — the source simply
/// becomes a run of no-break spaces — and the formula is painted over the gap.
/// Doing it this way keeps normal wrapping, and keeps the rest of the inline
/// formatting (bold, links, code) working around it.
fn expand_inline_math(
    inline: &inline::Inline,
    size: f32,
    color: Hsla,
    window: &mut Window,
) -> (
    String,
    Vec<(Range<usize>, u8)>,
    Vec<(Range<usize>, math::MathBox)>,
    Vec<(Range<usize>, Range<usize>)>,
) {
    // The ranges come from the scanner rather than from the style spans: it is
    // the one that knows `$$` is display maths and that `\$` is a dollar sign.
    // Inline code is handed over as well, so a literal `$` inside a code span
    // does not open a formula that runs to the next real one.
    let literal: Vec<Range<usize>> = inline
        .spans
        .iter()
        .filter(|(_, flags)| flags & inline::CODE != 0)
        .map(|(range, _)| range.clone())
        .collect();
    let sources: Vec<Range<usize>> = math::inline_spans(&inline.text, &literal)
        .into_iter()
        .map(|(range, _)| range)
        .collect();
    if sources.is_empty() {
        return (
            inline.text.clone(),
            inline.spans.clone(),
            Vec::new(),
            Vec::new(),
        );
    }

    // One no-break space of this font, to size the gap in characters.
    let space = window
        .text_system()
        .shape_text(
            SharedString::from("\u{a0}".to_string()),
            px(size),
            &[TextRun {
                len: '\u{a0}'.len_utf8(),
                font: Font {
                    family: ui_font().into(),
                    ..Default::default()
                },
                color: black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
            None,
        )
        .ok()
        .and_then(|lines| lines.first().map(|line| line.width().as_f32()))
        .filter(|width| *width > 0.5)
        .unwrap_or(size * 0.26);

    let mut out = String::with_capacity(inline.text.len());
    let mut gaps: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    let mut maths: Vec<(Range<usize>, math::MathBox)> = Vec::new();
    let mut cursor = 0usize;

    for range in &sources {
        out.push_str(&inline.text[cursor..range.start]);
        let source = inline.text[range.clone()].trim_matches('$');
        let math = math::layout(source, size, false, ui_font(), color, window);
        let blanks = ((math.width / space).ceil() as usize).max(1);
        let start = out.len();
        for _ in 0..blanks {
            out.push('\u{a0}');
        }
        let gap = start..out.len();
        maths.push((gap.clone(), math));
        gaps.push((range.clone(), gap));
        cursor = range.end;
    }
    out.push_str(&inline.text[cursor..]);

    let mut spans: Vec<(Range<usize>, u8)> = Vec::new();
    for (range, flags) in &inline.spans {
        if flags & inline::MATH != 0 {
            continue;
        }
        let start = boundary(&out, shift_past(range.start, &gaps));
        let end = boundary(&out, shift_past(range.end, &gaps));
        if start < end {
            spans.push((start..end, *flags));
        }
    }
    (out, spans, maths, gaps)
}

/// Pull an offset back onto a character boundary. The gaps are runs of
/// no-break spaces, so a span edge that landed inside a formula can end up in
/// the middle of one — the text shaper refuses to take that.
fn boundary(text: &str, mut at: usize) -> usize {
    if at >= text.len() {
        return text.len();
    }
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Drop the chip markers that fall inside a copied slice.
///
/// `base` is where the slice starts in the whole paragraph, so the marks — which
/// speak the paragraph's coordinates — can be lined up with it. A marker only
/// half inside the slice is left alone: the copy starts or ends inside the chip,
/// and cutting a character off it would be worse than keeping it.
fn strip_marks(slice: &str, base: usize, marks: &[Range<usize>]) -> String {
    let mut out = String::with_capacity(slice.len());
    let mut cursor = base;
    for mark in marks {
        if mark.start < cursor || mark.end > base + slice.len() {
            continue;
        }
        out.push_str(&slice[cursor - base..mark.start - base]);
        cursor = mark.end;
    }
    out.push_str(&slice[cursor - base..]);
    out
}

/// Map a byte offset in the original text to the same place after the gaps were
/// swapped in.
fn shift_past(position: usize, gaps: &[(Range<usize>, Range<usize>)]) -> usize {
    let mut delta: isize = 0;
    for (old, new) in gaps {
        if position >= old.end {
            delta += new.len() as isize - old.len() as isize;
        } else if position > old.start {
            return new.start + (position - old.start).min(new.len());
        } else {
            break;
        }
    }
    ((position as isize + delta).max(0)) as usize
}

/// What the parentheses of a link point at, turned into something the desktop
/// can open. A bare `notes/说明.md` is resolved against the folder of the
/// document, which is what the author of the link meant by it.
fn resolve_address(url: &str, base: &Path) -> Option<String> {
    let lowered = url.to_ascii_lowercase();
    if ["http://", "https://", "mailto:", "file://"]
        .iter()
        .any(|scheme| lowered.starts_with(scheme))
    {
        return Some(url.to_string());
    }
    // `Path::join` keeps an absolute path as it is, so both cases work.
    let path = base.join(Path::new(url));
    if path.is_file() {
        return Some(path.to_string_lossy().to_string());
    }
    // `www.example.com/docs`, written without a scheme: the shell would take it
    // for a file name, so put the scheme back. Names that end in a file
    // extension are a link to a file that is not there yet, not a host.
    const FILE_ENDINGS: &[&str] = &[
        "md", "markdown", "txt", "png", "jpg", "jpeg", "gif", "svg", "webp", "pdf", "json", "toml",
        "yaml", "yml", "rs", "py", "js", "ts", "html", "css",
    ];
    let host = url.split('/').next().unwrap_or_default();
    let ending = host.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    let looks_like_a_site = host.is_ascii()
        && host.contains('.')
        && !host.starts_with('.')
        && ending.len() >= 2
        && !FILE_ENDINGS.contains(&ending.as_str())
        && !url.contains('\\');
    looks_like_a_site.then(|| format!("https://{url}"))
}

/// Hand `target` to the desktop. The shell is the only thing that knows which
/// browser, or which editor, the user actually wants.
fn open_externally(target: &str) {
    #[cfg(target_os = "windows")]
    {
        // `start` runs through the shell, so associations apply. The empty
        // argument fills the window-title slot `start` insists on — without it
        // the address would be swallowed as that title.
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", target])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(target).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(target).spawn();
    }
}

/// Paints a paragraph's own text with the inline runs the Markdown scanner
/// found: bold, italics, strikethrough, a link's blue, and a soft fill behind
/// inline code.
///
/// A field draws its text itself, so handing it a string is not enough — the
/// moment the caret lands in a paragraph the styling would vanish. This is the
/// seam the field offers for colouring its runs, and the runs are refreshed by
/// the view just before the field is laid out.
struct InlineHighlighter {
    runs: Rc<RefCell<Vec<(Range<usize>, HighlightStyle)>>>,
}

impl InputHighlighter for InlineHighlighter {
    fn language(&self) -> SharedString {
        "markdown-inline".into()
    }

    fn update(
        &mut self,
        _: Option<InputEdit>,
        _: &Rope,
        _: bool,
        _: &mut Window,
        _: &mut Context<EditorState>,
    ) {
        // Nothing to parse: the runs arrive from the Markdown scanner.
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let runs = self.runs.borrow();
        let mut out: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        let mut cursor = range.start;
        for (span, style) in runs.iter() {
            let start = span.start.max(range.start);
            let end = span.end.min(range.end);
            if start >= end {
                continue;
            }
            if start > cursor {
                out.push((cursor..start, HighlightStyle::default()));
            }
            out.push((start..end, *style));
            cursor = end;
        }
        if cursor < range.end {
            out.push((cursor..range.end, HighlightStyle::default()));
        }
        out
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

/// Turn the runs into highlight styles.
///
/// The chip's markers are ordinary characters in the rendered text — that is
/// where the chip gets its padding, since a painted chip can never push a word
/// along. They are blanks while the caret is elsewhere, so they need no style
/// of their own; `opened` turns them back into backticks.
///
/// A chip is emitted as *three* runs — blank, word, blank — even though all
/// three carry the same style. The reason is the code face: a highlight style
/// can recolour a run but not change its family, and the family override the
/// text element takes only lands on runs it covers whole. Handing the shaper
/// one run for the whole chip would leave that run sticking out past the
/// override on both ends, the override would be skipped, and the pill (which
/// *is* measured in the code face) would then be wider than the word inside it
/// — which is exactly how a chip ends up with its word pushed to the left and
/// its right edge over the punctuation that follows.
fn styled_runs(
    spans: &[(Range<usize>, u8)],
    theme: &ThemeColor,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut runs = Vec::with_capacity(spans.len());
    for (range, flags) in spans {
        let style = inline_style(*flags, theme);
        match chip_parts(range) {
            Some(parts) if flags & inline::CODE != 0 => {
                for part in parts {
                    runs.push((part, style.clone()));
                }
            }
            _ => runs.push((range.clone(), style)),
        }
    }
    runs
}

fn styled_inline(text: &str, spans: &[(Range<usize>, u8)], theme: &ThemeColor) -> StyledText {
    let spans = styled_runs(spans, theme);
    let mut styled = StyledText::new(SharedString::from(text.to_string()));
    if !spans.is_empty() {
        styled = styled.with_highlights(spans);
    }
    styled
}

/// The bytes of a chip that are its code, as opposed to the two blanks that
/// hold its padding open.
///
/// A highlight style can recolour a run but cannot set its family — it carries
/// colour, weight and decoration only. The text element can, though, be told a
/// family for a range of text, and a range only takes effect where it covers a
/// whole run. The blanks are one byte each and are runs of their own, so the
/// content is what sits one byte in from either edge.
fn chip_content(chip: &Range<usize>) -> Option<Range<usize>> {
    (chip.end.saturating_sub(chip.start) > 2).then(|| chip.start + 1..chip.end - 1)
}

/// A chip cut into the three runs it has to reach the shaper as.
///
/// The look is the same across all three — what differs is the face, and the
/// face cannot be said in a highlight: the text element takes a family override
/// per byte range and only applies it to runs it covers whole. One run for the
/// whole chip therefore gets no code face at all, while the pill behind it is
/// measured with one — which is how a chip ends up with its word pushed left
/// and its right edge lying over the punctuation after it.
fn chip_parts(chip: &Range<usize>) -> Option<[Range<usize>; 3]> {
    let content = chip_content(chip)?;
    Some([
        chip.start..content.start,
        content.clone(),
        content.end..chip.end,
    ])
}

/// Hand every chip's text to the code face.
///
/// Code inside a sentence reads as code because of what it looks like as much
/// as because of the grey pill: the same face the fences use, so `&mut` and
/// `0x1F` sit on the same grid in both places. The padding blanks are left in
/// the prose's face on purpose — they are measured in the prose's em, which is
/// what keeps the chip the same width whichever words are inside it.
fn with_code_face(styled: StyledText, chips: &[Range<usize>]) -> StyledText {
    let overrides = chip_faces(chips);
    if overrides.is_empty() {
        styled
    } else {
        styled.with_font_family_overrides(overrides)
    }
}

/// Every chip on a line, as ranges of the rendered text.
fn code_chips(spans: &[(Range<usize>, u8)]) -> Vec<Range<usize>> {
    spans
        .iter()
        .filter(|(_, flags)| flags & inline::CODE != 0)
        .map(|(range, _)| range.clone())
        .collect()
}

/// The runs that have to reach the shaper in the code face: the content of each
/// chip, without the two blanks that hold its padding open.
fn chip_faces(chips: &[Range<usize>]) -> Vec<(Range<usize>, SharedString)> {
    chips
        .iter()
        .filter_map(chip_content)
        .map(|content| (content, SharedString::from(mono_font())))
        .collect()
}

/// The line cut into the runs the shaper needs: prose in the UI face, inline
/// code in the code face.
///
/// The overlay and the text element both go through this. A chip's pill is
/// placed from glyph positions, so if the two disagreed about which face a run
/// is set in they would disagree about where every character after it starts —
/// and the pill would slide off its word on any line that carries one.
fn line_runs(text: &str, bold: bool, chips: &[Range<usize>]) -> Vec<TextRun> {
    let face = |code: bool| Font {
        family: if code { mono_font() } else { ui_font() }.into(),
        weight: if bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        },
        ..Default::default()
    };
    let run = |len: usize, code: bool| TextRun {
        len,
        font: face(code),
        color: black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };

    let mut runs = Vec::new();
    let mut cursor = 0;
    for content in chips.iter().filter_map(chip_content) {
        if content.start > cursor {
            runs.push(run(content.start - cursor, false));
        }
        runs.push(run(content.len(), true));
        cursor = content.end;
    }
    if cursor < text.len() {
        runs.push(run(text.len() - cursor, false));
    }
    runs
}

/// The same runs, but an inline `code` span carries its own background.///
/// A paragraph paints rounded chips in an overlay above the text; a table cell
/// has no overlay, so a code span there would come out as plain text. Painting
/// the fill as part of the run keeps the chip — square, but visible — wherever
/// the text is drawn.
fn styled_inline_chip(
    text: &str,
    spans: &[(Range<usize>, u8)],
    theme: &ThemeColor,
) -> StyledText {
    let chip = code_chip(theme);
    let codes: Vec<&Range<usize>> = spans
        .iter()
        .filter(|(_, flags)| flags & inline::CODE != 0)
        .map(|(range, _)| range)
        .collect();
    let spans: Vec<(Range<usize>, HighlightStyle)> = styled_runs(spans, theme)
        .into_iter()
        .map(|(range, mut style)| {
            if codes
                .iter()
                .any(|code| code.start <= range.start && range.end <= code.end)
            {
                style.background_color = Some(chip);
            }
            (range, style)
        })
        .collect();
    let mut styled = StyledText::new(SharedString::from(text.to_string()));
    if !spans.is_empty() {
        styled = styled.with_highlights(spans);
    }
    styled
}

/// Where the things that sit *on* a line of text belong: rounded chips behind
/// inline `code`, formulas that replace their `$…$` source, and the box each
/// link occupies so a Ctrl+click can find it.
#[derive(Default)]
struct Overlay {
    chips: Vec<Bounds<Pixels>>,
    /// (x, top) of every formula, in the order they were given.
    maths: Vec<(f32, f32)>,
    /// (index of the link it came from, its box) — a link that wraps produces
    /// one box per line it touches, so the index is carried along.
    links: Vec<(usize, Bounds<Pixels>)>,
}

/// Air between a chip's own glyphs and its rounded edge.
///
/// The padding itself comes from the backticks, which are real characters in
/// the line; this is only what keeps the corner from clipping them.
const CHIP_PAD: f32 = 1.5;
/// Corner of a chip.
const CHIP_RADIUS: f32 = 4.0;
/// How tall a chip is, as a multiple of its text size.
///
/// The pill is painted *behind* the glyphs, so this number is nothing but the
/// air above and below them. At 1.38 a chip stood taller than the letters it
/// held and a line of prose read as a row of lozenges; 1.26 still clears a
/// descender while sitting inside the line rather than on top of it.
const CHIP_HEIGHT_RATIO: f32 = 1.26;

/// Shape the very same string at the very same width and read the glyph offsets
/// back out, so an overlay can be positioned over the text the element tree
/// lays out. The text engine is the only thing that knows where a span starts
/// once the paragraph has been wrapped.
///
/// `faces` are the runs that are set in the code face — the chips whose text
/// the line was actually shaped with — while `chips` are the ones that get a
/// pill drawn. The two are the same list almost always; they part company for
/// the chip the caret is inside, which keeps the code face but shows its
/// backticks instead of a fill.
#[allow(clippy::too_many_arguments)]
/// A label that goes somewhere: an invisible box over the link text, painted
/// after the words so a click finds it before the paragraph underneath.
///
/// `dx` shifts the box inside its parent — the field of a paragraph being
/// edited keeps a caret inset the resting prose does not have, and a link has
/// to sit exactly where its label was laid out in either of them.
fn link_hit(
    cx: &mut Context<MeowDown>,
    key: SharedString,
    rect: Bounds<Pixels>,
    url: String,
    dx: Pixels,
) -> AnyElement {
    div()
        .id(key)
        .absolute()
        .left(dx + rect.origin.x)
        .top(rect.origin.y)
        .w(rect.size.width)
        .h(rect.size.height)
        // The hand says "this one goes somewhere", which is the only affordance
        // a link has while the document is being read rather than written.
        .cursor_pointer()
        // Only Ctrl+click opens the address; a plain click has to reach the
        // block underneath so the caret still lands where the pointer is.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                if event.modifiers.control || event.modifiers.platform {
                    this.open_link(&url, window, cx);
                    cx.stop_propagation();
                }
            }),
        )
        .into_any_element()
}

fn overlay_geometry(
    text: &str,
    size: f32,
    bold: bool,
    width: Pixels,
    faces: &[Range<usize>],
    chips: &[Range<usize>],
    maths: &[(Range<usize>, f32)],
    links: &[Range<usize>],
    window: &mut Window,
) -> Overlay {
    let mut overlay = Overlay::default();
    if text.is_empty()
        || width <= px(8.)
        || (chips.is_empty() && maths.is_empty() && links.is_empty())
    {
        return overlay;
    }

    let runs = line_runs(text, bold, faces);
    let Ok(lines) = window.text_system().shape_text(
        SharedString::from(text.to_string()),
        px(size),
        &runs,
        Some(width),
        None,
    ) else {
        return overlay;
    };

    let line_height = size * LINE_RATIO;
    let chip_height = size * CHIP_HEIGHT_RATIO;
    // Byte offset of the current hard line inside the whole block.
    let mut base = 0usize;
    // Visual row across the whole block, so the vertical position keeps going.
    let mut row = 0usize;
    let last = lines.len().saturating_sub(1);

    for (line_index, line) in lines.iter().enumerate() {
        let layout = &line.unwrapped_layout;
        // Where each visual row starts: (byte within the line, x within the line).
        let mut rows: Vec<(usize, f32)> = vec![(0, 0.0)];
        for boundary in line.wrap_boundaries() {
            let glyph = layout
                .runs
                .get(boundary.run_ix)
                .and_then(|run| run.glyphs.get(boundary.glyph_ix));
            if let Some(glyph) = glyph {
                rows.push((glyph.index, glyph.position.x.as_f32()));
            }
        }

        for (row_index, (row_start, row_start_x)) in rows.iter().enumerate() {
            let row_end = rows
                .get(row_index + 1)
                .map(|(start, _)| *start)
                .unwrap_or(layout.len);
            let top = row as f32 * line_height;
            for span in chips {
                let start = span.start.saturating_sub(base);
                let end = span.end.saturating_sub(base);
                let from = start.max(*row_start);
                let to = end.min(row_end);
                if from >= to {
                    continue;
                }
                // `x_for_index` speaks in the coordinates of the *unwrapped*
                // line, and so does `row_start_x`, so the index stays absolute
                // and only the x is rebased onto the start of the row.
                let x0 = layout.x_for_index(from).as_f32() - row_start_x;
                let x1 = layout.x_for_index(to).as_f32() - row_start_x;
                // The chip covers the run's glyphs, backticks included. They
                // are what the padding is made of, so nothing here has to guess
                // at the neighbours: a chip is the same width on both sides of
                // its word, wherever it sits on the line. Two chips back to
                // back share an edge instead of each reaching into the other.
                let touching = |at: usize| text.as_bytes().get(at) == Some(&b'`');
                let left = if from == start && from > *row_start && !touching(base + from - 1) {
                    CHIP_PAD
                } else {
                    0.0
                };
                let right = if to == end && to < row_end && !touching(base + to) {
                    CHIP_PAD
                } else {
                    0.0
                };
                overlay.chips.push(Bounds {
                    origin: point(px(x0 - left), px(top + (line_height - chip_height) / 2.)),
                    size: gpui::size(px(x1 - x0 + left + right), px(chip_height)),
                });
            }
            for (span, baseline) in maths {
                let start = span.start.saturating_sub(base);
                if start < *row_start || start >= row_end {
                    continue;
                }
                let x = layout.x_for_index(start).as_f32() - row_start_x;
                // Line the formula's own baseline up with the text's: the text
                // is centred in the line box, so add that half-leading back.
                let text_baseline = top
                    + (line_height - layout.ascent.as_f32() - layout.descent.as_f32()) / 2.
                    + layout.ascent.as_f32();
                overlay.maths.push((x, text_baseline - baseline));
            }
            for (index, span) in links.iter().enumerate() {
                let from = span.start.saturating_sub(base).max(*row_start);
                let to = span.end.saturating_sub(base).min(row_end);
                if from >= to {
                    continue;
                }
                let x0 = layout.x_for_index(from).as_f32() - row_start_x;
                let x1 = layout.x_for_index(to).as_f32() - row_start_x;
                overlay.links.push((
                    index,
                    Bounds {
                        origin: point(px(x0), px(top)),
                        size: gpui::size(px((x1 - x0).max(1.)), px(line_height)),
                    },
                ));
            }
            row += 1;
        }

        if line_index < last {
            base += layout.len + 1;
        }
    }
    overlay
}

// ------------------------------------------------------------------ rendering

impl MeowDown {
    /// One command in a drop-down menu.
    ///
    /// The label and the shortcut are two columns of the row, not one padded
    /// string: the keys then line up on the right edge of every item instead of
    /// drifting with the length of the label.
    fn menu_action<F>(
        view: &Entity<MeowDown>,
        label: &str,
        keys: &str,
        run: F,
    ) -> PopupMenuItem
    where
        F: Fn(&mut MeowDown, &mut Window, &mut Context<MeowDown>) + 'static,
    {
        let view = view.clone();
        let label = SharedString::from(label.to_string());
        let keys = SharedString::from(keys.to_string());
        PopupMenuItem::element(move |_, cx| {
            let theme = cx.theme().colors;
            let mut row = div()
                .flex()
                .items_center()
                .w_full()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .whitespace_nowrap()
                        .child(label.clone()),
                );
            if !keys.is_empty() {
                row = row.child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(keys.clone()),
                );
            }
            row
        })
        .on_click(move |_, window, cx| {
            view.update(cx, |this, cx| run(this, window, cx));
        })
    }

    fn render_menubar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().colors;
        let auto_save = self.settings.auto_save;
        let view = cx.entity();

        let file = {
            let view = view.clone();
            let autosave_state = if auto_save { "已开启" } else { "已关闭" };
            Button::new("menu-file")
                .ghost()
                .small()
                .label("文件")
                .dropdown_menu(move |menu, _, _| {
                    menu.min_w(px(288.)).item(Self::menu_action(&view, "新建", "Ctrl+N", |this, window, cx| {
                        this.new_document(window, cx)
                    }))
                    .item(Self::menu_action(&view, "打开…", "Ctrl+O", |this, window, cx| {
                        this.open_document(window, cx)
                    }))
                    .item(Self::menu_action(
                        &view,
                        "打开文件夹…",
                        "Ctrl+Shift+O",
                        |this, window, cx| this.open_folder(window, cx),
                    ))
                    .item(Self::menu_action(
                        &view,
                        "关闭文件夹",
                        "",
                        |this, _window, cx| {
                            this.root = None;
                            this.folder_scan_at = None;
                            this.hint = "已关闭文件夹".into();
                            cx.notify();
                        },
                    ))
                    .separator()
                    .item(Self::menu_action(&view, "保存", "Ctrl+S", |this, window, cx| {
                        this.save_document(window, cx)
                    }))
                    .item(Self::menu_action(
                        &view,
                        "另存为…",
                        "Ctrl+Shift+S",
                        |this, window, cx| this.save_as(window, cx),
                    ))
                    .item(Self::menu_action(&view, "自动保存", autosave_state, |this, window, cx| {
                        this.toggle_auto_save(window, cx)
                    }))
                    .separator()
                    .item(Self::menu_action(&view, "设置…", "Ctrl+,", |this, window, cx| {
                        this.open_settings(SettingsTab::General, window, cx)
                    }))
                    .item(Self::menu_action(&view, "退出", "Ctrl+W", |this, window, cx| {
                        this.request_close(window, cx)
                    }))
                })
        };

        let edit = {
            let view = view.clone();
            Button::new("menu-edit")
                .ghost()
                .small()
                .label("编辑")
                .dropdown_menu(move |menu, _, _| {
                    menu.min_w(px(288.)).item(Self::menu_action(&view, "撤销", "Ctrl+Z", |this, window, cx| {
                        this.undo(window, cx)
                    }))
                    .item(Self::menu_action(&view, "重做", "Ctrl+Y", |this, window, cx| {
                        this.redo(window, cx)
                    }))
                    .separator()
                    .item(Self::menu_action(&view, "剪切", "Ctrl+X", |this, window, cx| {
                        this.cut(window, cx)
                    }))
                    .item(Self::menu_action(&view, "复制", "Ctrl+C", |this, _window, cx| {
                        this.copy_selection(cx)
                    }))
                    .item(Self::menu_action(&view, "粘贴", "Ctrl+V", |this, window, cx| {
                        this.paste(window, cx);
                    }))
                    .item(Self::menu_action(
                        &view,
                        "粘贴图片",
                        "",
                        |this, window, cx| {
                            this.paste_clipboard_image(window, cx);
                        },
                    ))
                    .separator()
                    .item(Self::menu_action(&view, "全选", "Ctrl+A", |this, window, cx| {
                        this.select_all(window, cx)
                    }))
                })
        };

        let paragraph = {
            let view = view.clone();
            Button::new("menu-paragraph")
                .ghost()
                .small()
                .label("段落")
                .dropdown_menu(move |menu, _, _| {
                    menu.min_w(px(288.)).item(Self::menu_action(&view, "正文", "Ctrl+0", |this, window, cx| {
                        this.apply_active_kind(BlockKind::Paragraph, window, cx)
                    }))
                    .separator()
                    .item(Self::menu_action(&view, "一级标题", "Ctrl+1", |this, window, cx| {
                        this.apply_active_kind(BlockKind::Heading(1), window, cx)
                    }))
                    .item(Self::menu_action(&view, "二级标题", "Ctrl+2", |this, window, cx| {
                        this.apply_active_kind(BlockKind::Heading(2), window, cx)
                    }))
                    .item(Self::menu_action(&view, "三级标题", "Ctrl+3", |this, window, cx| {
                        this.apply_active_kind(BlockKind::Heading(3), window, cx)
                    }))
                    .item(Self::menu_action(&view, "四级标题", "Ctrl+4", |this, window, cx| {
                        this.apply_active_kind(BlockKind::Heading(4), window, cx)
                    }))
                    .item(Self::menu_action(&view, "五级标题", "Ctrl+5", |this, window, cx| {
                        this.apply_active_kind(BlockKind::Heading(5), window, cx)
                    }))
                    .item(Self::menu_action(&view, "六级标题", "Ctrl+6", |this, window, cx| {
                        this.apply_active_kind(BlockKind::Heading(6), window, cx)
                    }))
                    .separator()
                    .item(Self::menu_action(
                        &view,
                        "提升标题级别",
                        "Ctrl+=",
                        |this, window, cx| this.shift_heading(-1, window, cx),
                    ))
                    .item(Self::menu_action(
                        &view,
                        "降低标题级别",
                        "Ctrl+-",
                        |this, window, cx| this.shift_heading(1, window, cx),
                    ))
                    .separator()
                    .item(Self::menu_action(
                        &view,
                        "表格",
                        "Ctrl+T",
                        |this, window, cx| this.insert_table(window, cx),
                    ))
                    .item(Self::menu_action(
                        &view,
                        "在下方插入一行",
                        "Ctrl+Enter",
                        |this, window, cx| this.add_table_row_here(window, cx),
                    ))
                    .item(Self::menu_action(
                        &view,
                        "删除当前行",
                        "Ctrl+Shift+Backspace",
                        |this, window, cx| this.remove_table_row_here(window, cx),
                    ))
                    .item(Self::menu_action(
                        &view,
                        "公式（行内 $…$ / 独立 $$…$$）",
                        "Ctrl+Shift+M",
                        |this, window, cx| this.toggle_math_block(window, cx),
                    ))
                    .item(Self::menu_action(
                        &view,
                        "代码块",
                        "Ctrl+Shift+K",
                        |this, window, cx| this.toggle_code_block(window, cx),
                    ))
                    .item(Self::menu_action(&view, "引用", "Ctrl+Shift+Q", |this, window, cx| {
                        this.apply_active_kind(BlockKind::Quote, window, cx)
                    }))
                    .item(Self::menu_action(&view, "无序列表", "Ctrl+Shift+]", |this, window, cx| {
                        this.apply_active_kind(BlockKind::Bullet, window, cx)
                    }))
                    .item(Self::menu_action(&view, "有序列表", "Ctrl+Shift+[", |this, window, cx| {
                        this.apply_active_kind(BlockKind::Ordered, window, cx)
                    }))
                    .item(Self::menu_action(&view, "任务列表", "Ctrl+Shift+X", |this, window, cx| {
                        this.apply_active_kind(BlockKind::Todo(false), window, cx)
                    }))
                    .separator()
                    .item(Self::menu_action(
                        &view,
                        "上移当前段落",
                        "Alt+↑",
                        |this, window, cx| this.move_block(-1, window, cx),
                    ))
                    .item(Self::menu_action(
                        &view,
                        "下移当前段落",
                        "Alt+↓",
                        |this, window, cx| this.move_block(1, window, cx),
                    ))
                })
        };

        let format = {
            let view = view.clone();
            Button::new("menu-format")
                .ghost()
                .small()
                .label("格式")
                .dropdown_menu(move |menu, _, _| {
                    menu.min_w(px(288.)).item(Self::menu_action(&view, "加粗", "Ctrl+B", |this, window, cx| {
                        this.wrap_selection("**", "**", "加粗", window, cx)
                    }))
                    .item(Self::menu_action(&view, "斜体", "Ctrl+I", |this, window, cx| {
                        this.wrap_selection("*", "*", "斜体", window, cx)
                    }))
                    .item(Self::menu_action(&view, "下划线", "Ctrl+U", |this, window, cx| {
                        this.wrap_selection("<u>", "</u>", "下划线", window, cx)
                    }))
                    .item(Self::menu_action(
                        &view,
                        "删除线",
                        "Alt+Shift+5",
                        |this, window, cx| this.wrap_selection("~~", "~~", "删除线", window, cx),
                    ))
                    .item(Self::menu_action(
                        &view,
                        "行内代码",
                        "Ctrl+Shift+`",
                        |this, window, cx| this.wrap_selection("`", "`", "code", window, cx),
                    ))
                    .separator()
                    .item(Self::menu_action(&view, "超链接", "Ctrl+K", |this, window, cx| {
                        this.wrap_selection("[", "](https://)", "链接文字", window, cx)
                    }))
                    .item(Self::menu_action(&view, "插入图片", "Ctrl+Shift+I", |this, window, cx| {
                        this.insert_image(window, cx)
                    }))
                    .separator()
                    .item(Self::menu_action(
                        &view,
                        "清除格式",
                        "Ctrl+\\",
                        |this, window, cx| this.clear_format(window, cx),
                    ))
                })
        };

        let view_menu = {
            let view = view.clone();
            Button::new("menu-view")
                .ghost()
                .small()
                .label("视图")
                .dropdown_menu(move |menu, _, _| {
                    menu.min_w(px(288.)).item(Self::menu_action(
                        &view,
                        "源码模式",
                        "Ctrl+/",
                        |this, window, cx| this.toggle_source(window, cx),
                    ))
                    .item(Self::menu_action(
                        &view,
                        "显示/隐藏侧边栏",
                        "Ctrl+Shift+L",
                        |this, _window, cx| this.toggle_sidebar(cx),
                    ))
                    .separator()
                    .item(Self::menu_action(&view, "文件列表", "", |this, _window, cx| {
                        this.set_sidebar_tab(SidebarTab::Files, cx)
                    }))
                    .item(Self::menu_action(&view, "标题大纲", "", |this, _window, cx| {
                        this.set_sidebar_tab(SidebarTab::Outline, cx)
                    }))
                    .separator()
                    .item(Self::menu_action(&view, "专注模式", "F8", |this, window, cx| {
                        this.focus_mode = !this.focus_mode;
                        cx.notify();
                        let _ = window;
                    }))
                    .item(Self::menu_action(&view, "全屏", "F11", |this, window, cx| {
                        window.toggle_fullscreen();
                        let _ = this;
                        let _ = cx;
                    }))
                    .separator()
                    .item(Self::menu_action(&view, "切换深浅色", "", |this, window, cx| {
                        this.toggle_theme(window, cx)
                    }))
                    .item(Self::menu_action(&view, "上传全部本地图片", "Ctrl+Shift+U", |this, window, cx| {
                        this.upload_all_images(window, cx)
                    }))
                })
        };

        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .h(px(30.))
            .px_2()
            .flex_shrink_0()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(file)
            .child(edit)
            .child(paragraph)
            .child(format)
            .child(view_menu)
            .child(div().flex_1())
            .into_any_element()
    }

    /// The switch at the top of the side panel: files on disk versus this
    /// document's headings. Reads as one connected control that fills the
    /// panel's width, with a thumb that slides between the two halves instead
    /// of blinking from one side to the other.
    fn render_topbar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().colors;
        let active = self.sidebar_tab;
        // On dark themes the raised half has to be lighter than the track, on
        // light ones the page color already is.
        let raised = if cx.theme().is_dark() {
            theme.secondary
        } else {
            theme.background
        };

        // The slide is advanced here, one frame at a time: `request_animation_frame`
        // keeps the frames coming until the thumb lands.
        let mut pos = self.tab_pos;
        if let Some(slide) = self.tab_anim {
            let (value, done) = slide.value_at(Instant::now());
            pos = value;
            self.tab_pos = value;
            if done {
                self.tab_anim = None;
            } else {
                window.request_animation_frame();
            }
        }

        const TRACK_PAD: f32 = 3.0;
        const OUTER_PAD: f32 = 8.0;
        let inner = (SIDEBAR_WIDTH - OUTER_PAD * 2. - TRACK_PAD * 2.).max(1.);
        let cell = inner / 2.;

        let label =
            |text: &'static str, value: SidebarTab, cx: &mut Context<Self>| -> AnyElement {
                let selected = active == value;
                div()
                    .id(SharedString::from(text))
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .h(px(24.))
                    .rounded(px(6.))
                    .text_sm()
                    .when(selected, |this| {
                        this.text_color(theme.foreground)
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!selected, |this| {
                        this.text_color(theme.muted_foreground)
                            .hover(|style| style.text_color(theme.foreground))
                    })
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_sidebar_tab(value, cx)),
                    )
                    .child(text)
                    .into_any_element()
            };

        div()
            .flex()
            .items_center()
            .h(px(38.))
            .px(px(OUTER_PAD))
            .flex_shrink_0()
            .border_b_1()
            .border_color(theme.sidebar_border)
            .bg(theme.sidebar)
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .w_full()
                    .h(px(30.))
                    .p(px(TRACK_PAD))
                    .rounded(px(8.))
                    .bg(theme.muted)
                    .child(
                        div()
                            .absolute()
                            .left(px(cell * pos))
                            .top_0()
                            .w(px(cell))
                            .h_full()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(raised),
                    )
                    .child(label("文件", SidebarTab::Files, cx))
                    .child(label("大纲", SidebarTab::Outline, cx)),
            )
            .into_any_element()
    }

    fn render_sidebar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().colors;
        let tab = self.sidebar_tab;
        // Only the file list touches the disk, and it caches the result. The
        // cache is read by reference: copying a listing of a few thousand notes
        // on every frame is a cost that shows up as lag between keystrokes.
        if tab == SidebarTab::Files {
            self.refresh_siblings();
        }
        let tabs = self.render_topbar(_window, cx);
        let files = (tab == SidebarTab::Files).then(|| self.folder_files.as_slice());
        let (items, footer, scroll_id): (Vec<AnyElement>, AnyElement, &'static str) = match files {
            Some(files) => {
                // A tree shows a level at a time, so the number under it can
                // only speak for the level that was read: the root's own files.
                let count = match &self.root {
                    Some(_) => format!("根目录 {} 个 Markdown 文件", files.len()),
                    None => format!("{} 个 Markdown 文件", files.len()),
                };
                (
                    match &self.tree {
                        Some(tree) => self.tree_items(tree, 0, &theme, cx),
                        None => self.file_items(files, &theme, cx),
                    },
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .border_t_1()
                        .border_color(theme.sidebar_border)
                        .px_3()
                        .py_3()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(count)
                        .child(match &self.root {
                            Some(_) => "点文件夹展开 · 点文件打开",
                            None => "点击切换文档",
                        })
                        .into_any_element(),
                    "files-scroll",
                )
            }
            None => {
                let text = self.document_text();
                let word_count = model::count_words(&text);
                let char_count = text.chars().count();
                let minutes = (word_count as f32 / 350.0).ceil().max(1.0) as usize;
                (
                    self.outline_items(&theme, cx),
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .border_t_1()
                        .border_color(theme.sidebar_border)
                        .px_3()
                        .py_3()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("{word_count} 字 · {char_count} 字符"))
                        .child(format!("约 {minutes} 分钟阅读"))
                        .into_any_element(),
                    "outline-scroll",
                )
            }
        };

        div()
            .flex()
            .flex_col()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(theme.sidebar_border)
            .bg(theme.sidebar)
            .child(tabs)
            .child(
                div()
                    .id(scroll_id)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(px(1.))
                    .pt_2()
                    .overflow_y_scroll()
                    .children(items),
            )
            .child(footer)
            .into_any_element()
    }

    fn outline_items(&self, theme: &ThemeColor, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let headings = self.outline();
        let active = self.active_id;
        let mut items: Vec<AnyElement> = Vec::new();
        if headings.is_empty() {
            items.push(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("还没有标题。用 H1 / H2 建立文档结构。")
                    .into_any_element(),
            );
        }
        for (id, level, text) in headings {
            let indent = 12.0 + (level.saturating_sub(1) as f32) * 14.0;
            let is_active = active == Some(id);
            items.push(
                div()
                    .id(SharedString::from(format!("outline-{id}")))
                    .flex()
                    .items_center()
                    .w_full()
                    .pl(px(indent))
                    .pr_3()
                    .py(px(5.))
                    .rounded_md()
                    .text_sm()
                    .when(is_active, |this| {
                        this.bg(theme.sidebar_accent)
                            .text_color(theme.sidebar_accent_foreground)
                    })
                    .when(!is_active, |this| {
                        this.text_color(if level <= 2 {
                            theme.foreground
                        } else {
                            theme.muted_foreground
                        })
                    })
                    .hover(|style| style.bg(theme.sidebar_accent))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.activate(id, window, cx)),
                    )
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(if text.is_empty() {
                                SharedString::from("（空标题）")
                            } else {
                                text
                            }),
                    )
                    .into_any_element(),
            );
        }
        items
    }

    fn file_items(
        &self,
        files: &[PathBuf],
        theme: &ThemeColor,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if files.is_empty() {
            return vec![
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("这个文件夹里还没有 Markdown 文件。")
                    .into_any_element(),
            ];
        }

        let current = self.path.clone();
        let root = self.root.clone();
        files
            .iter()
            .map(|path| {
                let is_current = current.as_deref() == Some(path.as_path());
                let target = path.clone();
                // Inside an opened folder the name alone is ambiguous — two
                // `README.md` in different subfolders — so show the path from
                // the folder down.
                let (label, folder) = match &root {
                    Some(root) => {
                        let relative = relative_label(root, path);
                        match relative.rsplit_once('/') {
                            Some((dir, name)) => (name.to_string(), Some(dir.to_string())),
                            None => (relative, None),
                        }
                    }
                    None => (file_name(path), None),
                };
                div()
                    .id(path_id(path))
                    .flex()
                    .flex_col()
                    .w_full()
                    .pl(px(10.))
                    .pr_3()
                    .py(px(6.))
                    .gap(px(1.))
                    .rounded_md()
                    .text_sm()
                    .when(is_current, |this| {
                        this.bg(theme.sidebar_accent)
                            .text_color(theme.sidebar_accent_foreground)
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!is_current, |this| this.text_color(theme.sidebar_foreground))
                    .hover(|style| style.bg(theme.sidebar_accent))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.open_from_sidebar(target.clone(), window, cx)
                        }),
                    )
                    .child(
                        div()
                            .w_full()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(label),
                    )
                    .when_some(folder, |this, folder| {
                        this.child(
                            div()
                                .pl(px(13.))
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(folder),
                        )
                    })
                    .into_any_element()
            })
            .collect()
    }

    /// The opened folder as a tree: a row per folder with a chevron that opens
    /// it, and a row per Markdown file under the folder it sits in.
    fn tree_items(
        &self,
        node: &TreeNode,
        depth: usize,
        theme: &ThemeColor,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows: Vec<AnyElement> = Vec::new();
        // The root row would repeat the window title bar's path, so the list
        // starts with the folder's children.
        for child in &node.dirs {
            let open = self.expanded.contains(&child.path);
            // `None` until the folder has been read, which is what a collapsed
            // branch is: a number there would mean walking it after all.
            let count = child.file_count();
            // A folder with nothing inside cannot be opened: it gets no
            // chevron and a dimmed name, so an empty branch reads as empty
            // instead of as a file that failed to load.
            let hollow = child.shows_nothing();
            let path = child.path.clone();
            // A folder answers the right button whether it is empty or not: the
            // menu is where "新建文件" lives, so an empty folder is exactly the
            // one that needs it.
            let menu_path = child.path.clone();
            rows.push(
                div()
                    .id(path_id(&child.path))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .w_full()
                    .pl(px(8. + 12. * depth as f32))
                    .pr_3()
                    .py(px(4.))
                    .rounded_md()
                    .text_sm()
                    .text_color(if hollow {
                        theme.muted_foreground
                    } else {
                        theme.sidebar_foreground
                    })
                    .when(!hollow, |this| {
                        this.cursor_pointer()
                            .hover(|style| style.bg(theme.sidebar_accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if this.expanded.remove(&path) {
                                        cx.notify();
                                        return;
                                    }
                                    this.expanded.insert(path.clone());
                                    // Read the level being opened, now: the next
                                    // frame draws whatever is in it.
                                    this.sync_tree();
                                    cx.notify();
                                }),
                            )
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.open_menu(menu_path.clone(), true, event.position, cx);
                        }),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(px(10.))
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(match (hollow, open) {
                                (true, _) => "",
                                (false, true) => "▾",
                                (false, false) => "▸",
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_weight(FontWeight::MEDIUM)
                            .child(child.name.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .children(count.filter(|count| *count > 0).map(|count| count.to_string())),
                    )
                    .into_any_element(),
            );
            if open {
                rows.extend(self.tree_items(child, depth + 1, theme, cx));
            }
        }
        for file in &node.files {
            rows.push(self.tree_file_row(file, depth, theme, cx));
        }
        rows
    }

    fn tree_file_row(
        &self,
        path: &Path,
        depth: usize,
        theme: &ThemeColor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_current = self.path.as_deref() == Some(path);
        let target = path.to_path_buf();
        let menu_target = path.to_path_buf();
        div()
            .id(path_id(path))
            .flex()
            .items_center()
            .gap_2()
            .w_full()
            .pl(px(20. + 12. * depth as f32))
            .pr_3()
            .py(px(5.))
            .rounded_md()
            .text_sm()
            .when(is_current, |this| {
                this.bg(theme.sidebar_accent)
                    .text_color(theme.sidebar_accent_foreground)
                    .font_weight(FontWeight::MEDIUM)
            })
            .when(!is_current, |this| this.text_color(theme.sidebar_foreground))
            .hover(|style| style.bg(theme.sidebar_accent))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.open_from_sidebar(target.clone(), window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_menu(menu_target.clone(), false, event.position, cx);
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(file_name(path)),
            )
            .into_any_element()
    }

    // ------------------------------------------------ file tree: right-click

    /// Open the right-click menu for `path`, at the pointer.
    fn open_menu(&mut self, path: PathBuf, is_dir: bool, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.menu = Some(FileMenu { path, is_dir, at });
        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    /// The menu panel. It floats over the whole window at the click, and closes
    /// on any press outside itself.
    /// The list of languages, hung under a fence's language field.
    ///
    /// It is drawn at the root rather than inside the block because a block
    /// clips what leaves it, and a list a couple of hundred pixels tall would be
    /// cut in half by the very code it belongs to.
    fn render_lang_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().colors;
        let Some(block) = self.lang_menu.as_ref().map(|menu| menu.block) else {
            return div().into_any_element();
        };
        // The block may have been folded away or the document replaced while the
        // list was up. A panel anchored to a field that is no longer drawn would
        // still cover the window and swallow every press.
        if self.index_of(block).is_none() {
            self.lang_menu = None;
            return div().into_any_element();
        }
        let (active, scroll, rows) = {
            let menu = self.lang_menu.as_ref().expect("checked just above");
            (menu.active, menu.scroll, menu.rows.clone())
        };
        let Some(field) = self.lang_bounds.borrow().get(&block).copied() else {
            // Nothing has been painted yet, so there is no field to hang the
            // list off; the next frame will have drawn one.
            return div().into_any_element();
        };

        let shown = rows.len().clamp(1, LANG_ROWS_SHOWN);
        let height = px(shown as f32 * LANG_ROW + 8.);
        let viewport = window.viewport_size();
        // Right edges line up: the panel ends where the box ends, so it reads as
        // belonging to it.
        let left = (field.right() - px(LANG_PANEL_WIDTH))
            .min((viewport.width - px(LANG_PANEL_WIDTH) - px(8.)).max(px(8.)))
            .max(px(8.));
        let below = field.bottom() + px(6.);
        let top = if below + height + px(8.) <= viewport.height {
            below
        } else {
            // No room underneath: the list goes above the field instead of
            // hanging off the bottom of the window.
            (field.top() - height - px(6.)).max(px(8.))
        };

        let mut items: Vec<AnyElement> = Vec::with_capacity(rows.len().max(1));
        if rows.is_empty() {
            items.push(
                div()
                    .flex()
                    .items_center()
                    .h(px(LANG_ROW))
                    .px(px(12.))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("没有匹配的语言")
                    .into_any_element(),
            );
        }
        for (index, choice) in rows.into_iter().enumerate() {
            let name = choice.name.clone();
            let token = choice.token.clone();
            let picked = choice;
            items.push(
                div()
                    .id(SharedString::from(format!("lang-row-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(10.))
                    .w_full()
                    .h(px(LANG_ROW))
                    .mx(px(4.))
                    .px(px(8.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(index == active, |this| this.bg(theme.sidebar_accent))
                    .hover(|style| style.bg(theme.sidebar_accent))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.foreground)
                            .child(SharedString::from(name)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(mono_font())
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(token)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.choose_lang(picked.clone(), window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .into_any_element(),
            );
        }

        div()
            .id("lang-menu")
            .absolute()
            .inset_0()
            // Any press that is not on a row is a press on the page, and the
            // list is over: a picker that survives a click elsewhere is a
            // picker that is in the way. The field itself is the exception —
            // the list hangs off it, and pressing it is aiming at the anchor,
            // not dismissing the panel.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    let on_field = this
                        .lang_bounds
                        .borrow()
                        .get(&block)
                        .is_some_and(|area| {
                            event.position.x >= area.left()
                                && event.position.x <= area.right()
                                && event.position.y >= area.top()
                                && event.position.y <= area.bottom()
                        });
                    if !on_field {
                        this.close_lang_menu(cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| this.close_lang_menu(cx)),
            )
            // The wheel belongs to the list while it is up, so a couple of
            // hundred languages can be browsed without typing a letter.
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                let delta = event.delta.pixel_delta(window.line_height()).y;
                if let Some(menu) = this.lang_menu.as_mut() {
                    let total = menu.rows.len() as f32 * LANG_ROW;
                    let view = LANG_ROWS_SHOWN as f32 * LANG_ROW;
                    let limit = (total - view).max(0.);
                    menu.scroll = (menu.scroll - delta.as_f32()).clamp(0., limit);
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .child(
                div()
                    .id("lang-menu-panel")
                    .absolute()
                    .left(left)
                    .top(top)
                    .w(px(LANG_PANEL_WIDTH))
                    .py(px(4.))
                    .flex()
                    .flex_col()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .shadow(picture_shadow(&theme))
                    .child(
                        div().h(px(shown as f32 * LANG_ROW)).overflow_hidden().child(
                            div()
                                .flex()
                                .flex_col()
                                // Scrolled by hand: the rows are a slice of the
                                // list, moved by the wheel and by the keyboard.
                                .mt(px(-scroll))
                                .children(items),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().colors;
        let Some(menu) = self.menu.clone() else {
            return div().into_any_element();
        };
        let target = menu.path.clone();
        let is_dir = menu.is_dir;
        // "新建…" writes into a folder: the folder itself, or the one the file
        // sits in. Same for a search.
        let dir = if is_dir {
            target.clone()
        } else {
            target
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| target.clone())
        };

        let mut rows: Vec<AnyElement> = Vec::new();
        if !is_dir {
            rows.push(menu_item("打开", &theme, cx, {
                let path = target.clone();
                move |this, window, cx| this.open_from_sidebar(path.clone(), window, cx)
            }));
            rows.push(menu_item("在新窗口中打开", &theme, cx, {
                let path = target.clone();
                move |this, _, cx| this.open_in_new_window(path.clone(), cx)
            }));
        }
        rows.push(menu_item("新建文件", &theme, cx, {
            let dir = dir.clone();
            move |this, window, cx| this.new_file_in(dir.clone(), window, cx)
        }));
        rows.push(menu_item("新建文件夹", &theme, cx, {
            let dir = dir.clone();
            move |this, window, cx| this.new_folder_in(dir.clone(), window, cx)
        }));
        if is_dir {
            rows.push(menu_item("搜索", &theme, cx, {
                let dir = dir.clone();
                move |this, window, cx| this.start_search(dir.clone(), window, cx)
            }));
        }
        rows.push(menu_rule(&theme));
        rows.push(menu_item("重命名", &theme, cx, {
            let path = target.clone();
            move |this, window, cx| this.start_rename(path.clone(), window, cx)
        }));
        if !is_dir {
            rows.push(menu_item("创建副本", &theme, cx, {
                let path = target.clone();
                move |this, _, cx| this.duplicate_file(path.clone(), cx)
            }));
        }
        rows.push(menu_rule(&theme));
        rows.push(menu_item("删除", &theme, cx, {
            let path = target.clone();
            move |this, window, cx| this.confirm_delete(path.clone(), window, cx)
        }));
        rows.push(menu_item("属性", &theme, cx, {
            let path = target.clone();
            move |this, _, cx| this.show_properties(path.clone(), cx)
        }));
        rows.push(menu_rule(&theme));
        rows.push(menu_item("复制文件路径", &theme, cx, {
            let path = target.clone();
            move |this, _, cx| this.copy_path(path.clone(), cx)
        }));
        rows.push(menu_item("打开文件位置", &theme, cx, {
            let path = target.clone();
            move |this, _, cx| this.reveal_file(path.clone(), cx)
        }));

        // A menu opened near the right or bottom edge would hang off the window,
        // so it is pulled back inside instead of being cut in half.
        let height = px(rows.len() as f32 * MENU_ROW + 8.);
        let viewport = window.viewport_size();
        let x = menu
            .at
            .x
            .min((viewport.width - px(MENU_WIDTH) - px(8.)).max(px(8.)));
        let y = menu
            .at
            .y
            .min((viewport.height - height - px(8.)).max(px(8.)));

        div()
            .id("file-menu")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close_menu(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| this.close_menu(cx)),
            )
            .child(
                div()
                    .id("file-menu-panel")
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(px(MENU_WIDTH))
                    .flex()
                    .flex_col()
                    .py(px(4.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .children(rows),
            )
            .into_any_element()
    }

    // ------------------------------------------------------ file tree: prompts

    fn close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt.take().is_some() {
            // The modal took the keyboard for its own field. Handing it back is
            // part of closing: otherwise the page keeps looking selected while
            // it takes no typing at all.
            self.return_to_document(window, cx);
            cx.notify();
        }
    }

    /// The question the window asks on its way out: 保存 / 丢弃 / 取消.
    ///
    /// It is drawn above everything else, and the dim room around it answers
    /// "取消" — a stray click on the page was never meant as "throw it away".
    fn render_close_prompt(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().colors;
        let focus = self.close_focus.clone();
        let named = self.path.as_ref().map(|path| file_name(path));
        let (title, note) = match &named {
            Some(name) => (
                "保存对这份文档的修改？",
                format!("「{name}」里还有没写回磁盘的内容，关闭之后这些修改就没了。"),
            ),
            None => (
                "保存这份文档？",
                "这份文档还没有保存到磁盘，关闭之后写下的内容就没了。".to_string(),
            ),
        };

        div()
            .id("close-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 0.35,
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.cancel_close(window, cx)),
            )
            // The wheel stops here too, for the same reason the settings
            // dialog stops it: the page behind must not move while a question
            // about it is on screen.
            .on_scroll(|_: &ScrollEvent, _: &mut Window, cx: &mut App| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .id("close-card")
                    .track_focus(&focus)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w(px(392.))
                    .p_4()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    // Clicks inside the card are not clicks on the dim room.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(note),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .w_full()
                            .child(prompt_button("保存", true, &theme, cx, |this, window, cx| {
                                this.save_and_close(window, cx)
                            }))
                            .child(prompt_button("丢弃", false, &theme, cx, |this, window, cx| {
                                this.discard_and_close(window, cx)
                            }))
                            .child(prompt_button("取消", false, &theme, cx, |this, window, cx| {
                                this.cancel_close(window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    /// Give the keyboard back to the page after something else held it.
    ///
    /// A freshly created file is loaded and then renamed, and while the rename
    /// box is up the document has no caret. Without this the box closes onto a
    /// page that is silent: typing goes nowhere until the reader clicks a line.
    fn return_to_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocks.is_empty() {
            return;
        }
        let index = self
            .active_id
            .and_then(|id| self.index_of(id))
            .unwrap_or(0);
        self.focus_block(index, window, cx);
    }

    /// The modal a file-tree command opened: a name to type, a delete to
    /// confirm, a search, or the properties card.
    fn render_prompt(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().colors;

        // A search walks the disk, so it is re-run when the query changes and
        // not on every frame the window paints.
        if let Some(prompt) = self.prompt.as_mut() {
            if prompt.kind == PromptKind::Search {
                let query = prompt
                    .field
                    .as_ref()
                    .map(|field| field.read(cx).value().to_string())
                    .unwrap_or_default();
                if query != prompt.hits_query {
                    prompt.hits = search_hits(&prompt.path, &query);
                    prompt.hits_query = query;
                }
            }
        }

        let Some(prompt) = self.prompt.as_ref() else {
            return div().into_any_element();
        };
        let kind = prompt.kind;
        let title = prompt.title.clone();
        let path = prompt.path.clone();
        let note = prompt.note.clone();
        let rows_data = prompt.rows.clone();
        let hits = prompt.hits.clone();
        let field = prompt.field.clone();
        let width = if matches!(kind, PromptKind::Properties | PromptKind::Search) {
            460.
        } else {
            400.
        };

        let mut body: Vec<AnyElement> = Vec::new();
        body.push(
            div()
                .text_base()
                .font_weight(FontWeight::MEDIUM)
                .child(title)
                .into_any_element(),
        );
        body.push(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(path.display().to_string())
                .into_any_element(),
        );
        if let Some(field) = &field {
            body.push(
                div()
                    .w_full()
                    .px_2()
                    .py(px(5.))
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .child(
                        Textarea::new(field)
                            .appearance(false)
                            .bordered(false)
                            .w_full()
                            .p_0()
                            .text_size(px(13.))
                            .text_color(theme.foreground),
                    )
                    .into_any_element(),
            );
        }
        for (label, value) in &rows_data {
            body.push(
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .w_full()
                    .text_sm()
                    .child(
                        div()
                            .w(px(76.))
                            .flex_shrink_0()
                            .text_color(theme.muted_foreground)
                            .child(label.clone()),
                    )
                    .child(div().flex_1().min_w_0().child(value.clone()))
                    .into_any_element(),
            );
        }
        if kind == PromptKind::Search {
            let mut list = div()
                .id("search-hits")
                .flex()
                .flex_col()
                .gap(px(1.))
                .w_full()
                .max_h(px(260.))
                .overflow_y_scroll();
            if hits.is_empty() {
                let empty = if path.display().to_string().is_empty() {
                    "输入关键字"
                } else {
                    "这个文件夹里没有匹配的文件"
                };
                list = list.child(
                    div()
                        .px_2()
                        .py_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(empty),
                );
            }
            for hit in &hits {
                let target = hit.clone();
                let label = relative_label(&path, hit);
                list = list.child(
                    div()
                        .id(SharedString::from(format!("hit:{}", hit.display())))
                        .flex()
                        .items_center()
                        .w_full()
                        .px_2()
                        .py(px(4.))
                        .rounded(px(5.))
                        .text_sm()
                        .text_color(theme.foreground)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.sidebar_accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.prompt = None;
                                this.load_from_path(target.clone(), window, cx);
                                cx.notify();
                            }),
                        )
                        .child(label),
                );
            }
            body.push(list.into_any_element());
        }
        if !note.is_empty() {
            body.push(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(note)
                    .into_any_element(),
            );
        }

        let mut buttons: Vec<AnyElement> = Vec::new();
        match kind {
            PromptKind::Properties => {
                buttons.push(prompt_button("关闭", false, &theme, cx, |this, window, cx| {
                    this.close_prompt(window, cx)
                }));
            }
            PromptKind::Delete => {
                buttons.push(prompt_button("取消", false, &theme, cx, |this, window, cx| {
                    this.close_prompt(window, cx)
                }));
                buttons.push(prompt_button("删除", true, &theme, cx, |this, window, cx| {
                    this.commit_prompt(window, cx)
                }));
            }
            PromptKind::Search => {
                buttons.push(prompt_button("关闭", false, &theme, cx, |this, window, cx| {
                    this.close_prompt(window, cx)
                }));
            }
            PromptKind::Rename => {
                buttons.push(prompt_button("取消", false, &theme, cx, |this, window, cx| {
                    this.close_prompt(window, cx)
                }));
                buttons.push(prompt_button("确定", true, &theme, cx, |this, window, cx| {
                    this.commit_prompt(window, cx)
                }));
            }
        }

        div()
            .id("prompt-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 0.35,
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_prompt(window, cx)),
            )
            .child(
                div()
                    .id("prompt-card")
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w(px(width))
                    .p_4()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    // Clicks inside the card are not clicks on the backdrop.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .children(body)
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .w_full()
                            .children(buttons),
                    ),
            )
            .into_any_element()
    }

    /// Do whatever the open prompt asked for. Enter and the primary button both
    /// land here, so the keyboard and the mouse cannot drift apart.
    fn commit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        match prompt.kind {
            PromptKind::Rename => self.apply_rename(prompt, cx),
            PromptKind::Delete => self.apply_delete(prompt, window, cx),
            PromptKind::Search => {
                if let Some(first) = prompt.hits.first().cloned() {
                    self.load_from_path(first, window, cx);
                }
            }
            PromptKind::Properties => {}
        }
        // A prompt that did not go through puts itself back on screen and keeps
        // the keyboard; one that is gone hands it to the page.
        if self.prompt.is_none() {
            self.return_to_document(window, cx);
        }
        cx.notify();
    }

    /// Rename the file or folder the prompt is holding.
    fn apply_rename(&mut self, prompt: Prompt, cx: &mut Context<Self>) {
        let raw = prompt
            .field
            .as_ref()
            .map(|field| field.read(cx).value().to_string())
            .unwrap_or_default();
        let name = raw.trim().to_string();
        if let Some(problem) = name_problem(&name) {
            self.reopen_with_note(prompt, problem, cx);
            return;
        }
        let parent = prompt
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let target = parent.join(&name);
        if target == prompt.path {
            self.hint = "名字没有变".into();
            return;
        }
        if target.exists() {
            self.reopen_with_note(prompt, "这个名字已经有人用了".to_string(), cx);
            return;
        }
        match std::fs::rename(&prompt.path, &target) {
            Ok(()) => {
                // The document on screen may be the one that just moved.
                if self.path.as_deref() == Some(prompt.path.as_path()) {
                    self.path = Some(target.clone());
                }
                // So may the folder the panel is listing.
                if let Some(root) = self.root.clone() {
                    if root == prompt.path {
                        self.root = Some(target.clone());
                    }
                }
                self.refresh_folder(cx);
                self.hint = format!("已重命名为 {}", file_name(&target)).into();
            }
            Err(err) => self.reopen_with_note(prompt, format!("重命名失败：{err}"), cx),
        }
    }

    /// Put the prompt back on screen with the reason it did not go through.
    fn reopen_with_note(&mut self, mut prompt: Prompt, note: String, cx: &mut Context<Self>) {
        prompt.note = note.into();
        self.prompt = Some(prompt);
        cx.notify();
    }

    /// Send the file or folder to the Recycle Bin, once the user has said so.
    fn apply_delete(&mut self, prompt: Prompt, window: &mut Window, cx: &mut Context<Self>) {
        match shellops::recycle(&prompt.path) {
            Ok(()) => {
                let open_document = self
                    .path
                    .as_ref()
                    .is_some_and(|open| open.starts_with(&prompt.path));
                let opened_folder = self
                    .root
                    .as_ref()
                    .is_some_and(|root| root.starts_with(&prompt.path));
                if opened_folder {
                    self.root = None;
                }
                if open_document {
                    // The file is gone: forget the path first, or the blank
                    // document that replaces it would be written straight back
                    // into the folder that no longer has it.
                    self.path = None;
                    self.dirty = false;
                }
                self.refresh_folder(cx);
                if open_document {
                    self.new_document(window, cx);
                }
                self.hint = format!("已删除 {}（可从回收站找回）", file_name(&prompt.path)).into();
            }
            Err(err) => self.hint = format!("删除失败：{err}").into(),
        }
    }

    fn start_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = file_name(&path);
        let field = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 1)
                .submit_on_enter(false)
        });
        field.update(cx, |field, cx| field.set_value(name, window, cx));
        self.take_keyboard(field.clone(), window, cx);
        self.prompt = Some(Prompt {
            kind: PromptKind::Rename,
            path,
            title: "重命名".into(),
            field: Some(field),
            note: "".into(),
            hits: Vec::new(),
            hits_query: String::new(),
            rows: Vec::new(),
        });
        cx.notify();
    }

    fn start_search(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let field = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 1)
                .submit_on_enter(false)
                .placeholder("文件名关键字")
        });
        self.take_keyboard(field.clone(), window, cx);
        self.prompt = Some(Prompt {
            kind: PromptKind::Search,
            path: dir,
            title: "搜索".into(),
            field: Some(field),
            note: "回车打开第一个结果".into(),
            hits: Vec::new(),
            hits_query: String::new(),
            rows: Vec::new(),
        });
        cx.notify();
    }

    /// Hand the keyboard to a prompt's field once the frame that opened it has
    /// settled.
    ///
    /// Focusing it on the spot is not enough. A prompt can be opened right after
    /// a document was loaded, and loading one asks the first paragraph to take
    /// the focus again at the end of the cycle — that trailing request arrives
    /// after this one and quietly wins, so the box is on screen with a caret in
    /// it while every keystroke goes into the page behind. Asking later than the
    /// page does is what makes the box actually own the keyboard.
    fn take_keyboard(&self, field: Entity<TextareaState>, window: &mut Window, cx: &mut Context<Self>) {
        cx.defer_in(window, move |this, window, cx| {
            // A prompt that has been dismissed in the meantime must not pull
            // the focus back out of the page.
            if this.prompt.is_some() {
                field.update(cx, |field, cx| {
                    field.focus(window, cx);
                    // The suggested name is there to be replaced, the way
                    // Explorer leaves a fresh file's name selected: typing
                    // starts a new name instead of appending to the old one.
                    field.select_all(window, cx);
                });
            }
        });
    }

    fn confirm_delete(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        let name = file_name(&path);
        let title = if path.is_dir() {
            format!("删除文件夹「{name}」？")
        } else {
            format!("删除「{name}」？")
        };
        self.prompt = Some(Prompt {
            kind: PromptKind::Delete,
            path,
            title: title.into(),
            field: None,
            note: "会放进回收站，需要的话可以从那里找回来。".into(),
            hits: Vec::new(),
            hits_query: String::new(),
            rows: Vec::new(),
        });
        cx.notify();
    }

    fn show_properties(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let is_dir = path.is_dir();
        let mut rows: Vec<(String, String)> = Vec::new();
        rows.push(("名称".into(), file_name(&path)));
        rows.push((
            "位置".into(),
            path.parent()
                .map(|parent| parent.display().to_string())
                .unwrap_or_default(),
        ));
        rows.push((
            "类型".into(),
            if is_dir {
                "文件夹".to_string()
            } else {
                describe_kind(&path)
            },
        ));
        if let Ok(meta) = std::fs::metadata(&path) {
            if is_dir {
                let (files, folders) = count_children(&path);
                rows.push((
                    "内容".into(),
                    format!("{files} 个文件，{folders} 个文件夹"),
                ));
            } else {
                rows.push((
                    "大小".into(),
                    format!(
                        "{}（{} 字节）",
                        shellops::format_size(meta.len()),
                        meta.len()
                    ),
                ));
            }
            rows.push((
                "修改时间".into(),
                shellops::modified(&path).unwrap_or_else(|| "—".into()),
            ));
            rows.push((
                "创建时间".into(),
                shellops::created(&path).unwrap_or_else(|| "—".into()),
            ));
        }
        if !is_dir {
            if let Ok(text) = std::fs::read_to_string(&path) {
                rows.push((
                    "字数".into(),
                    format!(
                        "{} 字 · {} 字符 · {} 行",
                        model::count_words(&text),
                        text.chars().count(),
                        text.lines().count()
                    ),
                ));
            }
        }
        self.prompt = Some(Prompt {
            kind: PromptKind::Properties,
            path,
            title: "属性".into(),
            field: None,
            note: "".into(),
            hits: Vec::new(),
            hits_query: String::new(),
            rows,
        });
        cx.notify();
    }

    fn copy_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
        self.hint = "已复制路径".into();
        cx.notify();
    }

    fn reveal_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let is_dir = path.is_dir();
        let mut command = std::process::Command::new("explorer.exe");
        if is_dir {
            command.arg(&path);
        } else {
            // `/select,` needs the path in the same argument, comma and all.
            command.arg(format!("/select,{}", path.display()));
        }
        self.hint = match command.spawn() {
            Ok(_) => "已在资源管理器中打开".into(),
            Err(err) => format!("打开位置失败：{err}").into(),
        };
        cx.notify();
    }

    /// A second window on the same file, with its own caret and its own undo
    /// history. Both windows write the same file, which is what an editor that
    /// asks for "open in a new window" means.
    fn open_in_new_window(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let bounds = Bounds::centered(None, size(px(1240.), px(840.)), cx);
        let title = file_name(&path);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| cx.new(|cx| MeowDown::with_path(Some(path.clone()), window, cx)),
        );
        self.hint = match opened {
            Ok(_) => "已在新窗口打开".into(),
            Err(err) => format!("新窗口打开失败：{err}").into(),
        };
        cx.notify();
    }

    fn new_file_in(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path = unused_path(&dir, "新建文档", Some("md"));
        if let Err(err) = std::fs::write(&path, "") {
            self.hint = format!("新建文件失败：{err}").into();
            cx.notify();
            return;
        }
        self.expanded.insert(dir);
        self.refresh_folder(cx);
        self.load_from_path(path.clone(), window, cx);
        // The name it was born with is a placeholder; the caret is put in it
        // straight away so typing replaces it, the way Explorer does it.
        self.start_rename(path, window, cx);
    }

    fn new_folder_in(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path = unused_path(&dir, "新建文件夹", None);
        if let Err(err) = std::fs::create_dir(&path) {
            self.hint = format!("新建文件夹失败：{err}").into();
            cx.notify();
            return;
        }
        self.expanded.insert(dir);
        self.refresh_folder(cx);
        self.start_rename(path, window, cx);
    }

    fn duplicate_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let copy = copy_path_for(&path);
        self.hint = match std::fs::copy(&path, &copy) {
            Ok(_) => {
                self.refresh_folder(cx);
                format!("已创建副本 {}", file_name(&copy)).into()
            }
            Err(err) => format!("创建副本失败：{err}").into(),
        };
        cx.notify();
    }

    /// Forget the cached listing so the next frame re-reads the folder.
    fn refresh_folder(&mut self, cx: &mut Context<Self>) {
        self.folder_scan_at = None;
        self.tree = None;
        cx.notify();
    }

    fn render_body(
        &mut self,
        window: &mut Window,
        sidebar: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().colors;

        // A glide to a heading is advanced here: `request_animation_frame`
        // asks for the next frame, and each frame moves the offset a little
        // further along the curve. No timer, so it keeps up with the display.
        if let Some(anim) = self.scroll_anim {
            let (y, done) = anim.offset_at(Instant::now());
            let x = self.scroll.offset().x;
            self.scroll.set_offset(point(x, y));
            if done {
                self.scroll_anim = None;
            } else {
                window.request_animation_frame();
            }
        }

        // A link into another document lands here. The blocks are built the
        // moment the file is read, but the offset a glide is measured against
        // only exists once they have been painted — so the jump waits for the
        // frame that knows where the heading ended up.
        if let Some(anchor) = self.pending_anchor.clone() {
            match self.heading_block(&anchor) {
                Some(id) if self.bounds.borrow().contains_key(&id) => {
                    self.pending_anchor = None;
                    self.scroll_to_block(id);
                }
                Some(_) => window.request_animation_frame(),
                None => {
                    self.pending_anchor = None;
                    self.hint = format!("这份文档里没有「{anchor}」这个标题").into();
                    cx.notify();
                }
            }
        }

        if self.source_mode {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(theme.background)
                .child(
                    div()
                        .px_6()
                        .py_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("SOURCE · Ctrl+/ 回到所见即所得"),
                )
                .child(
                    div().flex_1().min_h_0().px_6().pb_6().child(
                        Textarea::new(&self.source)
                            .appearance(false)
                            .bordered(false)
                            .w_full()
                            .h_full()
                            .p_0()
                            .font_family(mono_font())
                            .text_size(px(14.))
                            .text_color(theme.foreground),
                    ),
                )
                .into_any_element();
        }

        // A definite width is what makes the text wrap: inside a scroll
        // container the available width is otherwise treated as unbounded.
        // It is resolved before the blocks are built so a code fence can count
        // its wrapped rows at the width it will actually get.
        //
        // The column takes 95% of the content area and grows with it; the
        // remaining 5% is split into the two margins, so the text is centred
        // and never runs edge to edge. The width here is the text width itself
        // — the air on either side comes from centering the column, not from
        // padding, so a block can measure the whole span it is given.
        let column = reading_column(window.viewport_size().width, sidebar);

        let ids: Vec<u64> = self.blocks.iter().map(|block| block.id).collect();
        let children: Vec<AnyElement> = ids
            .iter()
            .map(|id| self.render_block(*id, window, cx))
            .collect();

        let scroll = self.scroll.clone();
        // What the line above the page says. A folder with no note picked yet
        // gets its own wording: the sample's "直接开始编辑" would be a lie, and
        // a blank page with no explanation reads as a bug.
        let empty = self
            .blocks
            .iter()
            .all(|block| block.text.trim().is_empty());
        let banner: Option<&'static str> = if self.path.is_some() {
            None
        } else if self.sample {
            Some("示例文档 · 直接开始编辑，Ctrl+S 保存")
        } else if self.root.is_some() {
            empty.then_some("未打开文档 · 从左侧选一篇，或右键新建文件")
        } else {
            // An unnamed page once the sample has had its one showing: the
            // line introduces it while it is still empty, and gets out of the
            // way the moment there is something to read.
            empty.then_some("新文档 · 直接开始编辑，Ctrl+S 保存")
        };

        // The scrollbar is an overlay sibling of the scrolling area: inside it,
        // an absolutely positioned element would scroll along with the text.
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(theme.background)
            .child(
                div()
                    .id("page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    // The page is a click target in its own right: a press on
                    // the room around the text — the margins, the space under
                    // the last paragraph, the whole of an empty document — puts
                    // the caret in the nearest line instead of doing nothing.
                    // A press that landed on a block is left alone: those run
                    // their own handlers, and this one only runs after them.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.focus_page_at(event.position, window, cx);
                        }),
                    )
                    .child(
                        div()
                            .w(column)
                            .mx_auto()
                            .pt(px(34.))
                            .pb(px(180.))
                            .flex()
                            .flex_col()
                            .when_some(banner, |this, text| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .pb_4()
                                        .child(text),
                                )
                            })
                            .children(children),
                    ),
            )
            .child(Scrollbar::vertical(&scroll).mode(ScrollbarMode::Always))
            .into_any_element()
    }

    fn render_block(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().colors;
        let Some(index) = self.index_of(id) else {
            return div().into_any_element();
        };

        let kind = self.blocks[index].kind.clone();
        let is_active = self.active_id == Some(id);

        if kind.is_rule() {
            let at = index;
            return div()
                .id(SharedString::from(format!("rule-{id}")))
                .flex()
                .items_center()
                .gap_3()
                .px(px(BLOCK_PAD_X))
                .py_4()
                .child(div().flex_1().h(px(1.)).bg(theme.border))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .opacity(0.0)
                        .hover(|style| style.opacity(1.0))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| this.delete_block(at, window, cx)),
                        )
                        .child("删除"),
                )
                .into_any_element();
        }

        if kind.is_code() {
            return self.render_code_block(id, is_active, window, cx);
        }

        if kind.is_image() {
            return self.render_image_block(id, is_active, cx);
        }

        if kind.is_math() {
            return self.render_math_block(id, is_active, window, cx);
        }

        if kind.is_table() {
            return self.render_table_block(id, is_active, window, cx);
        }

        let text = self.blocks[index].text.to_string();
        let (size, bold, color) = block_type_style(&kind, &theme);
        let ordinal = self.ordinal_at(index);

        let marker: Option<AnyElement> = match &kind {
            BlockKind::Bullet | BlockKind::Ordered => {
                // A number needs room so `9.` and `10.` line up; a bullet needs
                // almost none, and a wide column there is exactly the gap that
                // made the marker look stranded from its text. Both are right
                // aligned, so the text always starts the same distance away.
                let (label, width) = if kind == BlockKind::Ordered {
                    (format!("{ordinal}."), 24.0)
                } else {
                    ("•".to_string(), 12.0)
                };
                Some(
                    div()
                        .w(px(width))
                        .flex_shrink_0()
                        .flex()
                        .justify_end()
                        .pt(px(1.))
                        .text_size(px(size * 0.92))
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(label))
                        .into_any_element(),
                )
            }
            BlockKind::Quote => Some(
                div()
                    .w(px(3.))
                    .flex_shrink_0()
                    .self_stretch()
                    .ml(px(-8.))
                    .mr(px(6.))
                    .rounded_full()
                    .bg(theme.primary)
                    .into_any_element(),
            ),
            BlockKind::Todo(checked) => {
                let at = index;
                let checked = *checked;
                Some(
                    div()
                        .w(px(22.))
                        .flex_shrink_0()
                        .pt(px(1.))
                        .child(
                            div()
                                .id(SharedString::from(format!("todo-{id}")))
                                .size(px(16.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(if checked { theme.primary } else { theme.border })
                                .bg(if checked { theme.primary } else { theme.background })
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(10.))
                                .text_color(theme.primary_foreground)
                                .cursor_pointer()
                                .child(if checked { "✓" } else { "" })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        this.toggle_todo(at, window, cx);
                                        cx.stop_propagation();
                                    }),
                                ),
                        )
                        .into_any_element(),
                )
            }
            _ => None,
        };

        // The same text metrics are used focused and unfocused, so clicking a
        // line never resizes it: only the tint of the block changes.
        let mut content = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .text_size(px(size))
            .line_height(relative(LINE_RATIO))
            .text_color(color);

        // Record where the text sits, so a click can be turned into a caret —
        // and so a link knows how wide the column it has to lay out in is.
        //
        // The probe is absolutely positioned: as a normal child it would be a
        // zero-height row at the top of the column, and everything downstream
        // that asks "how tall is this paragraph" — whether it wrapped, where to
        // glide the page — would be measuring nothing.
        let bounds_map = self.bounds.clone();
        let laid_out = self.bounds.clone();
        content = content.relative().child(
            canvas(
                move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
                    bounds_map.borrow_mut().insert(id, bounds);
                },
                |_: Bounds<Pixels>, _: (), _: &mut Window, _: &mut App| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );

        if is_active {
            let state = self.blocks[index].state.clone();
            // The bare editor element paints nothing by itself: it is the
            // component wrapper that projects the palette onto the state, and a
            // paragraph deliberately does not use that wrapper (it carries a
            // form field's padding). Unprojected, the caret and the selection
            // are laid down in a fully transparent colour — a field that takes
            // typing and shows neither where the caret is nor what is selected.
            {
                let theme = cx.theme();
                let style = InputEditorStyle {
                    foreground: theme.foreground,
                    muted_foreground: theme.muted_foreground,
                    background: theme.background,
                    border: theme.border,
                    selection: theme.selection,
                    caret: theme.caret,
                    highlight_styles: theme.highlight_theme.clone(),
                    ..Default::default()
                };
                state.update(cx, |state, _| state.set_editor_style(style));
            }
            // What the field should be showing. The prose keeps its markers
            // hidden; the one run the caret sits inside puts them back, so the
            // syntax appears where it is being edited and nowhere else.
            let selection = state.read(cx).selected_range();
            let current = self
                .blocks[index]
                .shown
                .clone()
                .unwrap_or_else(|| inline::render(&text));
            let edited = state.read(cx).value().to_string();
            // Rebuilding the prose writes into the field, so it is done only
            // when the caret has moved into or out of a run — that, and nothing
            // else, is what changes which markers are on show. Deriving the
            // prose afresh on every frame instead used to rewrite the field
            // between a keystroke and its change event, which threw the key
            // away. A field that no longer matches the prose it was given is
            // likewise left alone: the change event is on its way, and it is
            // the one that folds the edit into the document.
            let caret_src = current.source_point(selection.end);
            let caret = selection.is_empty().then_some(caret_src);
            let open_now = caret.and_then(|at| {
                inline::render(&text)
                    .element_at(at)
                    .map(|element| element.span.clone())
            });
            // Which run opens is only ever decided by a bare caret. Once there
            // is a selection the field has to be left exactly as it is, or the
            // rebuild would write prose back and drop the very selection the
            // pointer just made.
            let shown = if selection.is_empty()
                && edited == current.text
                && open_now != self.blocks[index].shown_open
            {
                let (shown, placed) = view_at(&text, caret_src);
                let value = shown.text.clone();
                state.update(cx, |state, cx| {
                    state.set_value(value, window, cx);
                    state.set_selected_range(placed..placed, cx);
                });
                self.blocks[index].shown_open = inline::render(&text)
                    .element_at(shown.source_point(placed))
                    .map(|element| element.span.clone());
                shown
            } else {
                current
            };
            self.blocks[index].shown = Some(shown.clone());
            // The field paints its own text, so the runs go to it before it is
            // laid out: `styles` is shared with its highlighter. Inline code
            // carries no fill of its own here: a run background is a square
            // slab with no padding, and it would sit under the rounded chip the
            // overlay paints — two shapes, one of them wrong. The chip is
            // painted below the text instead, exactly as it is while the
            // paragraph is at rest.
            let runs = styled_runs(&shown.spans, &theme);
            *self.blocks[index].styles.borrow_mut() = runs;
            // A chip the caret is sitting in has no fill: its backticks are on
            // show, the source is what is being edited, and a grey pill drawn
            // around `code` would read as a shape the text does not have. What
            // the pill gives up, the text takes back — the markers are two
            // blanks at rest and two backticks now, so the word keeps clear of
            // whatever sits beside it either way.
            let open_chip = shown.opened_chip();
            let all_chips = code_chips(&shown.spans);
            // The code face follows the caret into the field: the state is told
            // which runs of its text wear it, and the shaping splits the line
            // there. Without that the chip would be measured in the prose face
            // while it is being edited — a narrower pill, a word that jumps in
            // size, and a caret a few pixels off wherever a chip sits.
            let faces = chip_faces(&all_chips);
            state.update(cx, |state, cx| state.set_font_family_runs(faces, cx));
            let chip_ranges: Vec<Range<usize>> = all_chips
                .iter()
                .filter(|range| Some(*range) != open_chip.as_ref())
                .cloned()
                .collect();
            // The column the prose is laid out in. Rows are counted against a
            // slightly narrower measure than the field gets, so a rounding
            // difference can only ever leave slack at the bottom, never cut a
            // line off.
            let column = self
                .bounds
                .borrow()
                .get(&id)
                .map(|bounds| bounds.size.width.max(px(96.)))
                .unwrap_or(px(600.));
            let width = (column - px(8.)).max(px(80.));
            let (line_height, rows) = {
                let state = state.read(cx);
                let line_height = state
                    .line_height()
                    .filter(|height| *height > px(1.))
                    .unwrap_or(px(size * LINE_RATIO));
                // Only our own shaping counts here. The field's wrap map is a
                // frame behind on the first layout, and it also counts the
                // empty row a trailing newline leaves behind — a paragraph has
                // no hard breaks, so that row would just be a gap under the
                // text.
                let rows = prose_rows(&shown.text, size, bold, &all_chips, width, window).max(1);
                (line_height, rows)
            };
            // The bare editor element, not the one wrapped in an input frame:
            // that frame carries the padding of a form field, and a paragraph
            // has to start on the very pixel the un-focused prose starts on or
            // the line jumps the moment it is clicked into. What is left is a
            // small inset the editor keeps for its caret, taken back out of the
            // box so the text lines up with the paragraph above and below it.
            let mut field = div()
                .relative()
                .ml(-TEXT_INSET)
                .w(column + TEXT_INSET)
                .h(line_height * rows as f32);
            // The chips the paragraph shows while it is at rest are drawn here
            // as well. Left to itself the field would lay inline code out with
            // no fill at all, or with the square, unpadded slab a run
            // background gives — either way the chip changes shape the moment
            // the caret lands on the paragraph. Same geometry as the overlay
            // above, painted under the text, and measured with the same faces
            // the field was told to shape with. The chip the caret is inside is
            // not in this list at all: its backticks are what it shows.
            if !chip_ranges.is_empty() {
                let shaped_text = shown.text.clone();
                let shaped_faces = all_chips.clone();
                let shaped_chips = chip_ranges.clone();
                // The width the field itself wraps at: its own box, minus the
                // ten pixels the editor keeps on the right for the caret. Ask
                // for a different width here and the chips land on the wrong
                // words as soon as a paragraph wraps.
                let text_width = (column + TEXT_INSET - px(10.)).max(px(80.));
                field = field.child(
                    canvas(
                        move |_: Bounds<Pixels>, window: &mut Window, _: &mut App| {
                            overlay_geometry(
                                &shaped_text,
                                size,
                                bold,
                                text_width,
                                &shaped_faces,
                                &shaped_chips,
                                &[],
                                &[],
                                window,
                            )
                        },
                        move |bounds: Bounds<Pixels>,
                              overlay: Overlay,
                              window: &mut Window,
                              _: &mut App| {
                            for chip_bounds in overlay.chips {
                                paint_chip(
                                    window,
                                    Bounds {
                                        origin: bounds.origin + chip_bounds.origin,
                                        size: chip_bounds.size,
                                    },
                                    &theme,
                                );
                            }
                        },
                    )
                    .absolute()
                    .top_0()
                    .left(TEXT_INSET)
                    .w(text_width)
                    .h(line_height * rows as f32),
                );
            }
            field = field
                .font_family(ui_font())
                .text_size(px(size))
                .line_height(relative(LINE_RATIO))
                .text_color(color)
                .child(BaseEditor::new(&state));
            if bold {
                field = field.font_weight(FontWeight::BOLD);
            }
            // A link stays clickable while its paragraph is being edited. The
            // boxes are measured on the text the field is showing, at the width
            // and in the faces the field shapes with — the same call its chip
            // overlay makes — so the hand lands on the label, not beside it.
            let shown_links: Vec<Range<usize>> = shown
                .spans
                .iter()
                .filter(|(_, flags)| flags & inline::LINK != 0)
                .map(|(range, _)| range.clone())
                .collect();
            if !shown_links.is_empty() && shown_links.len() == shown.links.len() {
                let text_width = (column + TEXT_INSET - px(10.)).max(px(80.));
                let overlay = overlay_geometry(
                    &shown.text,
                    size,
                    bold,
                    text_width,
                    &all_chips,
                    &chip_ranges,
                    &[],
                    &shown_links,
                    window,
                );
                for (index, rect) in overlay.links {
                    let Some((_, url)) = shown.links.get(index) else {
                        continue;
                    };
                    let url = url.clone();
                    field = field.child(link_hit(
                        cx,
                        SharedString::from(format!("field-link-{id}-{index}")),
                        rect,
                        url,
                        TEXT_INSET,
                    ));
                }
            }
            // A file that was just created holds no words, so the page would be
            // a white sheet with a thin caret in the corner. One line of grey
            // text marks where that caret is and what the page is for; it goes
            // away with the first character typed.
            if text.is_empty() && self.blocks.len() == 1 {
                field = field.child(
                    div()
                        .absolute()
                        .top_0()
                        .left(TEXT_INSET)
                        .text_color(theme.muted_foreground)
                        .opacity(0.6)
                        .child(EMPTY_PAGE_HINT),
                );
            }
            content = content.child(field);
        } else {
            let inline = inline::render(&text);
            // Inline formulas take the place of their `$…$` source: the source
            // becomes blank space of the same width, and the typeset formula is
            // painted over that gap. The paragraph keeps the line breaks it
            // would have had, which is what keeps the page from reflowing as
            // soon as it contains maths.
            let (display, spans, maths, _gaps) =
                expand_inline_math(&inline, size, color, window);
            let chips: Vec<Range<usize>> = spans
                .iter()
                .filter(|(_, flags)| flags & inline::CODE != 0)
                .map(|(range, _)| range.clone())
                .collect();
            let geometry: Vec<(Range<usize>, f32)> = maths
                .iter()
                .map(|(range, math)| (range.clone(), math.baseline))
                .collect();
            let styled = with_code_face(styled_inline(&display, &spans, &theme), &chips);

            // Where the links ended up. The label is a run of text like any
            // other, so the only thing that can say where it sits on screen is
            // the shaper — the same call the overlay makes, run here because a
            // hit area has to be an element, and elements are built now, not
            // painted. The width comes from last frame's canvas; on the very
            // first frame there is nothing to measure against yet.
            let link_ranges: Vec<Range<usize>> = spans
                .iter()
                .filter(|(_, flags)| flags & inline::LINK != 0)
                .map(|(range, _)| range.clone())
                .collect();
            let mut hits: Vec<(Bounds<Pixels>, String)> = Vec::new();
            if !link_ranges.is_empty() && link_ranges.len() == inline.links.len() {
                if let Some(area) = laid_out.borrow().get(&id) {
                    let overlay = overlay_geometry(
                        &display,
                        size,
                        bold,
                        area.size.width,
                        &chips,
                        &chips,
                        &[],
                        &link_ranges,
                        window,
                    );
                    for (index, rect) in overlay.links {
                        if let Some((_, url)) = inline.links.get(index) {
                            hits.push((rect, url.clone()));
                        }
                    }
                } else if self.link_asked.borrow_mut().insert(id) {
                    // The column width only becomes known once the paragraph has
                    // been painted, so the first frame of a fresh document
                    // cannot know where its links are. Ask for one more frame,
                    // once, so the first Ctrl+click after opening a file is not
                    // a miss.
                    window.request_animation_frame();
                }
            }

            let mut layer = div().relative().w_full();
            if !chips.is_empty() || !maths.is_empty() {
                let boxes: Rc<Vec<math::MathBox>> =
                    Rc::new(maths.into_iter().map(|(_, math)| math).collect());
                let shaped_text = display.clone();
                let shaped_chips = chips.clone();
                let shaped_maths = geometry.clone();
                let painted = boxes.clone();
                layer = layer.child(
                    canvas(
                        move |bounds: Bounds<Pixels>, window: &mut Window, _: &mut App| {
                            overlay_geometry(
                                &shaped_text,
                                size,
                                bold,
                                bounds.size.width,
                                &shaped_chips,
                                &shaped_chips,
                                &shaped_maths,
                                &[],
                                window,
                            )
                        },
                        move |bounds: Bounds<Pixels>,
                              overlay: Overlay,
                              window: &mut Window,
                              cx: &mut App| {
                            for chip_bounds in overlay.chips {
                                paint_chip(
                                    window,
                                    Bounds {
                                        origin: bounds.origin + chip_bounds.origin,
                                        size: chip_bounds.size,
                                    },
                                    &theme,
                                );
                            }
                            for (index, (x, top)) in overlay.maths.iter().enumerate() {
                                if let Some(math) = painted.get(index) {
                                    math.paint(
                                        bounds.origin + point(px(*x), px(*top)),
                                        window,
                                        cx,
                                    );
                                }
                            }
                        },
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                );
            }
            layer = layer.child(styled);
            // The same hint for the resting page: the block can be empty and
            // out of focus — behind the colour picker, or right after the page
            // lost the caret to a dialog — and the page must still look like
            // something that takes typing.
            if text.is_empty() && self.blocks.len() == 1 {
                layer = layer.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .text_color(theme.muted_foreground)
                        .opacity(0.6)
                        .child(EMPTY_PAGE_HINT),
                );
            }
            for (index, (rect, url)) in hits.into_iter().enumerate() {
                layer = layer.child(link_hit(
                    cx,
                    SharedString::from(format!("link-{id}-{index}")),
                    rect,
                    url,
                    px(0.),
                ));
            }
            let mut rich = div().w_full().child(layer);
            if bold {
                rich = rich.font_weight(FontWeight::BOLD);
            }
            content = content.child(rich);
        }

        let list = kind == BlockKind::Bullet || kind == BlockKind::Ordered;
        let mut row = div().flex().w_full().items_start();
        if let Some(marker) = marker {
            // A list wants the marker close to its text; everything else keeps
            // the roomier gap that a quote bar or a checkbox reads better with.
            row = row.gap(px(if list { 5. } else { 8. })).child(marker);
        }
        if list {
            // A bullet pinned to the very edge of the page reads as a stray
            // dot, so a list starts a touch further in.
            row = row.pl(px(6.));
        }

        div()
            .id(SharedString::from(format!("block-{id}")))
            .flex()
            .flex_col()
            .w_full()
            .px(px(BLOCK_PAD_X))
            .pt(px(BLOCK_PAD_Y + block_space_above(&kind)))
            .pb(px(BLOCK_PAD_Y))
            .rounded_md()
            // Focus changes nothing but the caret: no tint, no ring, no extra
            // inset. Anything else makes the line jump as you click into it.
            .when(self.focus_mode && !is_active, |this| this.opacity(0.35))
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.activate_at(id, event.position, window, cx);
                    // The page behind the blocks has a handler of its own, for
                    // the empty room around them. A press that landed on a line
                    // is already resolved; letting it through would move the
                    // caret a second time, to the end of the paragraph.
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                // The moves keep coming after the button is released; only the
                // ones with it held down are part of a selection.
                if event.pressed_button.is_some() {
                    this.drag_over(id, event.position, window, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                    this.end_drag(cx);
                }),
            )
            .child(row.child(content))
            .into_any_element()
    }

    fn render_code_block(
        &mut self,
        id: u64,
        is_active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().colors;
        let Some(index) = self.index_of(id) else {
            return div().into_any_element();
        };
        let Some(lang) = self.blocks[index].lang.clone() else {
            return div().into_any_element();
        };
        let code = self.blocks[index].state.clone();
        // Whether the language box is the thing being typed into, which is what
        // makes its border stand up — and what puts the list under it.
        let focused = self.lang_focus == Some(id);

        // The editor is given a definite height, so the height has to match the
        // text it lays out — and it has to be the *same* every frame. Measuring
        // the painted text and feeding the difference back into the height did
        // that only by accident: once the box was a fraction of a pixel taller
        // than the text, the editor stretched its content to the viewport,
        // reported *that* as the text height, and the box grew another sliver
        // on the next frame. A few seconds of that and the page was visibly
        // sliding upwards. So the row count comes from the editor's own wrap
        // map, which describes the text alone, and the frame around it is a
        // constant.
        let text = self.blocks[index].text.clone();
        let fallback_rows = self.code_block_rows(id, index, window);
        let (line_height, rows) = {
            let code = self.blocks[index].state.read(cx);
            let line_height = code
                .line_height()
                .filter(|height| *height > px(1.))
                .unwrap_or(px(CODE_TEXT_SIZE * 1.5));
            // Two counts of the same thing: the editor's wrap map and our own
            // shaping. They agree almost always; when they do not, the larger
            // one is the safe answer, because a box that is too tall only has
            // invisible slack at the bottom while a box that is too short can
            // scroll — and an editor that can scroll takes the wheel away from
            // the page.
            let rows = code.display_map().wrap_row_count().max(fallback_rows).max(1);
            (line_height, rows)
        };
        let height = line_height * rows as f32 + px(CODE_CHROME);
        // A block that can scroll takes the wheel for itself: it moves the few
        // pixels it has, calls `stop_propagation`, and the page stands still
        // under the pointer. The slack in `CODE_CHROME` is what normally keeps
        // that from happening, but a wrap point can still land differently in
        // the editor than in our own shaping. If it did, the editor's scroll
        // offset is the proof: hand that displacement to the page and put the
        // editor back at the top, so the block never moves inside itself and
        // the wheel still moves the page.
        {
            let sliver = code.read(cx).scroll_offset().y;
            if sliver.as_f32().abs() >= 0.5 {
                let page = self.scroll.offset();
                let limit = self.scroll.max_offset().y.max(px(0.));
                let y = (page.y + sliver).clamp(-limit, px(0.));
                self.scroll.set_offset(point(page.x, y));
                code.update(cx, |code, cx| {
                    code.set_scroll_offset(point(px(0.), px(0.)), cx);
                });
            }
        }
        let editor = Editor::new(&code)
            .appearance(false)
            .bordered(false)
            .w_full()
            .h(height)
            .px(px(8.))
            // The editor adds its own inset at the bottom, so an even padding
            // here reads as a block that is top-heavy: give the top the space
            // and let the bottom keep its own.
            .pt(px(10.))
            .pb(px(2.))
            .font_family(mono_font())
            .text_size(px(CODE_TEXT_SIZE))
            .text_color(theme.foreground);

        div()
            .id(SharedString::from(format!("code-{id}")))
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .mt(px(6.))
            .mb(px(6.))
            .px(px(1.))
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(code_fill(&theme))
            .overflow_hidden()
            // The fence stands on the page as one box, and a press anywhere in
            // it belongs to the editor inside, never to the page behind.
            .child(self.block_probe(id))
            // A code block shows every line it has, so there is nothing inside
            // it to scroll: the wheel belongs to the page. Left alone, the
            // editor swallows it and the page stops moving under the pointer.
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                let delta = event.delta.pixel_delta(window.line_height()).y;
                if delta.is_zero() {
                    return;
                }
                // Offsets grow more negative downwards, exactly as the page's
                // own scroll handler treats them.
                let offset = this.scroll.offset();
                let limit = this.scroll.max_offset().y.max(px(0.));
                let y = (offset.y + delta).clamp(-limit, px(0.));
                this.scroll.set_offset(point(offset.x, y));
                cx.notify();
                cx.stop_propagation();
            }))
            .child(
                div()
                    .w_full()
                    .rounded_md()
                    .overflow_hidden()
                    // The card is as wide as the editor, so measuring it here
                    // tells us the width the rows actually wrap at.
                    .child(canvas(
                        {
                            let measured = self.code_rows.clone();
                            let entity_id = cx.entity_id();
                            move |bounds: Bounds<Pixels>, window: &mut Window, app: &mut App| {
                                let width =
                                    (bounds.size.width - px(CODE_TEXT_INSET)).max(px(80.));
                                let width = width.as_f32();

                                let mut hasher =
                                    std::collections::hash_map::DefaultHasher::new();
                                std::hash::Hash::hash(text.as_ref(), &mut hasher);
                                let text_hash = std::hash::Hasher::finish(&hasher);

                                // Steady state: nothing moved, so nothing to re-shape.
                                {
                                    let cache = measured.borrow();
                                    let same = cache.get(&id).is_some_and(|cached| {
                                        cached.text_hash == text_hash
                                            && (cached.width - width).abs() < 0.5
                                    });
                                    if same {
                                        return;
                                    }
                                }

                                let rows = wrapped_rows(&text, px(width), window);
                                measured.borrow_mut().insert(
                                    id,
                                    CodeRows {
                                        text_hash,
                                        width,
                                        rows,
                                    },
                                );
                                app.notify(entity_id);
                            }
                        },
                        |_: Bounds<Pixels>, _: (), _: &mut Window, _: &mut App| {},
                    ))
                    .child(editor),
            )
            // Typora puts the language in the bottom-right corner, and only
            // while the block is being edited.
            .child(
                div()
                    .absolute()
                    .bottom(px(5.))
                    .right(px(10.))
                    .when(!is_active, |this| this.opacity(0.0))
                    .child(
                        // The box is a select as much as a field, so it is drawn
                        // like one: a border, so it reads as something to click
                        // rather than as a label lying over the code, and a
                        // brighter one while it holds the keyboard.
                        div()
                            .relative()
                            .w(px(104.))
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(if focused { theme.primary } else { theme.border })
                            .bg(Hsla {
                                a: 0.94,
                                ..theme.background
                            })
                            .child(self.lang_probe(id))
                            .child(
                                Textarea::new(&lang)
                                    .appearance(false)
                                    .bordered(false)
                                    .w_full()
                                    .p_0()
                                    .font_family(mono_font())
                                    .text_size(px(11.))
                                    .text_color(theme.muted_foreground),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_image_block(
        &mut self,
        id: u64,
        is_active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().colors;
        let Some(index) = self.index_of(id) else {
            return div().into_any_element();
        };
        let text = self.blocks[index].text.clone();
        let Some(state) = self.blocks[index].plain.clone() else {
            return div().into_any_element();
        };
        let uploading = self.uploading.contains(&id);
        let image = model::parse_image(&text);
        let src = image.as_ref().map(|image| image.src.clone()).unwrap_or_default();
        let alt = image.as_ref().map(|image| image.alt.clone()).unwrap_or_default();

        let placeholder = |message: &'static str| -> AnyElement {
            div()
                .flex()
                .items_center()
                .justify_center()
                .w_full()
                .h(px(96.))
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.muted)
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(message)
                .into_any_element()
        };

        // Whether a picture is really drawn, or a placeholder stands in for it.
        // Only a picture gets a shadow and a double click; a placeholder keeps
        // the full width of the column, the way a media slot reads.
        let (media, is_picture): (AnyElement, bool) = if uploading {
            (placeholder("正在上传图片…"), false)
        } else if src.trim().is_empty() {
            (placeholder("图片路径为空"), false)
        } else if is_remote(&src) {
            match self.remote_image(&src, cx) {
                RemoteState::Ready(image) => (
                    img(image)
                        .max_w_full()
                        .max_h(px(460.))
                        .rounded_md()
                        .into_any_element(),
                    true,
                ),
                RemoteState::Pending => (placeholder("正在加载图片…"), false),
                RemoteState::Failed => (placeholder("图片下载失败，检查链接是否可访问"), false),
            }
        } else {
            let alt_for_fallback = alt.clone();
            let element = match self.resolve_image(&src) {
                Some(path) => img(path),
                None => img(SharedString::from(src.clone())),
            };
            (
                element
                    .max_w_full()
                    .max_h(px(460.))
                    .rounded_md()
                    .with_fallback(move || {
                        div()
                            .flex()
                            .items_center()
                            .w_full()
                            .min_h(px(64.))
                            .rounded_md()
                            .border_1()
                            .border_dashed()
                            .border_color(theme.border)
                            .px_3()
                            .py_2()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(if alt_for_fallback.is_empty() {
                                SharedString::from("图片加载失败")
                            } else {
                                SharedString::from(format!("图片加载失败：{}", alt_for_fallback))
                            })
                            .into_any_element()
                    })
                    .into_any_element(),
                true,
            )
        };

        // The picture stays visible while the block is focused: the Markdown
        // source is added above it, the way Typora puts the raw syntax of a
        // block above the block itself.
        let mut column = div()
            .flex()
            .flex_col()
            .w_full()
            .gap_1()
            .px(px(BLOCK_PAD_X))
            .py(px(8.))
            .rounded_md();

        if is_active {
            column = column.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pb_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(if uploading {
                                "正在上传，完成后自动替换为图床地址"
                            } else {
                                "图片与源码 · 双击图片看大图 · Esc 预览"
                            }),
                    )
                    .child(
                        Textarea::new(&state)
                            .appearance(false)
                            .bordered(false)
                            .w_full()
                            .p_0()
                            .font_family(mono_font())
                            .text_size(px(12.5))
                            .text_color(theme.muted_foreground),
                    ),
            );
        }

        // Pictures sit in the middle of the column, whatever their width.
        // Left to the column's own cross-axis alignment a narrow picture would
        // hug the left edge while a wide one filled the whole measure, so two
        // pictures on the same page did not line up with each other. The frame
        // around the picture shrinks to the picture itself, which keeps the
        // press area honest: double-clicking the room beside a narrow picture
        // is not double-clicking the picture.
        // A picture is wrapped in a frame that carries its shadow and answers
        // for its double click. The frame is exactly the picture's box, so the
        // shadow follows the picture's corners and the room beside a narrow
        // picture stays what it looks like: page, not picture.
        let picture: AnyElement = if is_picture {
            div()
                .id(SharedString::from(format!("picture-{id}")))
                .rounded_md()
                .shadow(picture_shadow(&theme))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        if event.click_count >= 2 {
                            this.picture_press = None;
                            if this.open_viewer(id, cx) {
                                cx.stop_propagation();
                                return;
                            }
                        } else {
                            // The press that may be the first half of a double
                            // click, and the one that opens the block for
                            // editing.
                            this.note_picture_press(id);
                        }
                        if this.active_id != Some(id) {
                            if let Some(index) = this.index_of(id) {
                                this.focus_block(index, window, cx);
                            }
                        }
                        cx.stop_propagation();
                    }),
                )
                .child(media)
                .into_any_element()
        } else {
            media
        };

        column = column.child(
            div()
                .w_full()
                .flex()
                .justify_center()
                .child(picture),
        );

        if !is_active && !alt.is_empty() {
            column = column.child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(alt)),
                    ),
            );
        }

        column
            .relative()
            .child(self.block_probe(id))
            .id(SharedString::from(format!("image-{id}")))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    // One press opens the block for editing — that is how the
                    // Markdown behind a picture is reached, and it has to stay
                    // on the same click it always was. The second press of a
                    // double click lands on the source row the first one just
                    // revealed, so it arrives here rather than at the picture;
                    // the remembered press is what tells it apart from typing.
                    let browsing = event.click_count >= 2
                        && this.picture_double_clicked(id)
                        && this.open_viewer(id, cx);
                    if !browsing && this.active_id != Some(id) {
                        if let Some(index) = this.index_of(id) {
                            this.focus_block(index, window, cx);
                        }
                    }
                    cx.stop_propagation();
                }),
            )
            .into_any_element()
    }

    /// The picture, full window, with its own controls.
    ///
    /// This is deliberately its own layer rather than a dialog: a viewer is
    /// something to move around in, so the wheel, the drag and the arrows all
    /// belong to it while it is up, and the document is not reachable behind.
    fn render_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(viewer) = self.viewer.as_ref() else {
            return div().into_any_element();
        };
        let viewport = window.viewport_size();
        let count = viewer.items.len();
        let at = viewer.at;
        let zoom = viewer.zoom;
        let offset = viewer.offset;
        let src = viewer.items[at].src.clone();
        let alt = viewer.items[at].alt.clone();

        // The layer is dark on purpose, whatever the app theme is: a picture is
        // judged against neutral black, and a light backdrop turns every dark
        // screenshot into a grey one.
        let shell = Hsla { h: 0.0, s: 0.0, l: 0.07, a: 1.0 };
        let ink = Hsla { h: 0.0, s: 0.0, l: 1.0, a: 0.92 };
        let dim = Hsla { h: 0.0, s: 0.0, l: 1.0, a: 0.6 };
        let faint = Hsla { h: 0.0, s: 0.0, l: 1.0, a: 0.4 };
        let pad = Hsla { h: 0.0, s: 0.0, l: 1.0, a: 0.1 };

        let message = |text: &'static str| -> AnyElement {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(dim)
                .child(text)
                .into_any_element()
        };

        let media: AnyElement = if is_remote(&src) {
            match self.remote_image(&src, cx) {
                RemoteState::Ready(image) => img(image)
                    .object_fit(ObjectFit::Contain)
                    .size_full()
                    .into_any_element(),
                RemoteState::Pending => message("正在加载图片…"),
                RemoteState::Failed => message("图片下载失败，检查链接是否可访问"),
            }
        } else {
            let element = match self.resolve_image(&src) {
                Some(path) => img(path),
                None => img(SharedString::from(src.clone())),
            };
            element
                .object_fit(ObjectFit::Contain)
                .size_full()
                .with_fallback(move || {
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(dim)
                        .child("图片加载失败")
                        .into_any_element()
                })
                .into_any_element()
        };

        // Where the picture is drawn. With its intrinsic size known the frame
        // is exactly the picture, which is what lets a click on the surrounding
        // room be told from a click on the picture. Without it the frame falls
        // back to the whole window and `object_fit` does the fitting.
        let picture = match self.image_size(&src) {
            Some(image) => {
                let area = viewer_bounds(image, viewport, zoom, offset);
                div()
                    .absolute()
                    .left(area.left())
                    .top(area.top())
                    .w(area.size.width)
                    .h(area.size.height)
                    .child(media)
                    .into_any_element()
            }
            None => div()
                .absolute()
                .inset_0()
                .p(px(VIEWER_PAD))
                .child(media)
                .into_any_element(),
        };

        let nav = |label: &'static str, side: bool| -> AnyElement {
            let step: i32 = if side { -1 } else { 1 };
            div()
                .id(SharedString::from(if side { "viewer-prev" } else { "viewer-next" }))
                .absolute()
                .top(px(0.))
                .h_full()
                .w(px(64.))
                .when(side, |this| this.left(px(0.)))
                .when(!side, |this| this.right(px(0.)))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_size(px(30.))
                .text_color(dim)
                .hover(|style| style.text_color(ink))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.viewer_step(step, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(label)
                .into_any_element()
        };

        let percent = (zoom * 100.0).round() as i32;

        div()
            .id("viewer")
            .absolute()
            .inset_0()
            .bg(shell)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if let Some(viewer) = this.viewer.as_mut() {
                        viewer.drag = Some(ViewerDrag {
                            from: event.position,
                            origin: viewer.offset,
                            moved: 0.0,
                        });
                    }
                    cx.notify();
                    // The document is still laid out underneath, and a press
                    // that reached it would open the picture a second time —
                    // which resets the zoom the drag was about to move.
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let Some(viewer) = this.viewer.as_mut() else {
                    return;
                };
                let Some(drag) = viewer.drag.as_mut() else {
                    return;
                };
                let dx = event.position.x - drag.from.x;
                let dy = event.position.y - drag.from.y;
                let travel = (dx.as_f32().powi(2) + dy.as_f32().powi(2)).sqrt();
                drag.moved = drag.moved.max(travel);
                viewer.offset = point(drag.origin.x + dx, drag.origin.y + dy);
                cx.notify();
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    let Some(viewer) = this.viewer.as_mut() else {
                        return;
                    };
                    let Some(drag) = viewer.drag.take() else {
                        return;
                    };
                    if drag.moved < VIEWER_CLICK_SLOP {
                        this.viewer_backdrop_clicked(event.position, window.viewport_size(), cx);
                    }
                    cx.notify();
                    cx.stop_propagation();
                }),
            )
            // The wheel is the zoom, and it stops here: the page behind must not
            // scroll while a picture is being looked at.
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                let delta = event.delta.pixel_delta(window.line_height()).y;
                if delta.is_zero() {
                    return;
                }
                let factor = (1.0 + delta.as_f32() / 600.0).clamp(0.5, 2.0);
                this.viewer_zoom(factor, event.position, window.viewport_size(), cx);
                cx.stop_propagation();
            }))
            .child(picture)
            .when(count > 1, |this| this.child(nav("‹", true)).child(nav("›", false)))
            .child(
                div()
                    .id("viewer-close")
                    .absolute()
                    .top(px(12.))
                    .right(px(14.))
                    .size(px(34.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_size(px(15.))
                    .text_color(dim)
                    .bg(pad)
                    .hover(|style| style.text_color(ink))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_viewer(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child("✕"),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(14.))
                    .left(px(0.))
                    .right(px(0.))
                    .flex()
                    .justify_center()
                    .child(
                        // The caption sits on a dark plate: a bright picture
                        // reaches the bottom of the window, and white text on
                        // white bands is no text at all.
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(2.))
                            .px(px(14.))
                            .py(px(6.))
                            .rounded_md()
                            .bg(Hsla { h: 0.0, s: 0.0, l: 0.0, a: 0.55 })
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(ink)
                                    .child(SharedString::from(format!("{at} / {count} · {percent}%", at = at + 1))),
                            )
                            .when(!alt.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(dim)
                                        .child(SharedString::from(alt.clone())),
                                )
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(faint)
                                    .child("滚轮缩放 · 拖动平移 · ← → 切换 · 0 复位 · Esc 关闭"),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// A `$$ … $$` formula: the typeset result, with its source above it while
    /// the block is being edited, so the formula previews as you type.
    fn render_math_block(
        &mut self,
        id: u64,
        is_active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().colors;
        let Some(index) = self.index_of(id) else {
            return div().into_any_element();
        };
        let source = self.blocks[index].text.to_string();
        let Some(state) = self.blocks[index].plain.clone() else {
            return div().into_any_element();
        };
        // Display maths is set slightly larger than body text, the way a book
        // sets an equation.
        let size = MATH_SIZE;
        let math = math::layout(&source, size, true, ui_font(), theme.foreground, window);
        let width = math.width.max(size);
        let height = math.height.max(size * 1.2);
        let formula = math_canvas(math, width, height);

        let mut column = div()
            .flex()
            .flex_col()
            .w_full()
            .gap_1()
            .px(px(BLOCK_PAD_X))
            .py(px(6.))
            .rounded_md();

        if is_active {
            column = column.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pb_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("公式源码 · $…$ 行内 / $$…$$ 独立成行"),
                    )
                    .child(
                        Textarea::new(&state)
                            .appearance(false)
                            .bordered(false)
                            .w_full()
                            .p_0()
                            .font_family(mono_font())
                            .text_size(px(12.5))
                            .text_color(theme.muted_foreground),
                    ),
            );
        }

        column = column.child(
            div()
                .w_full()
                .flex()
                .justify_center()
                .py(px(4.))
                .child(formula),
        );

        column
            .relative()
            .child(self.block_probe(id))
            .id(SharedString::from(format!("math-{id}")))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.activate(id, window, cx);
                    cx.stop_propagation();
                }),
            )
            .into_any_element()
    }

    /// A pipe table. Read-only it is a real grid; clicked, it shows its
    /// Markdown so the rows can be typed on directly.
    fn render_table_block(
        &mut self,
        id: u64,
        _is_active: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().colors;
        let Some(index) = self.index_of(id) else {
            return div().into_any_element();
        };
        let source = self.blocks[index].text.to_string();
        div()
            .w_full()
            .px(px(BLOCK_PAD_X))
            .py(px(6.))
            // A table opens a cell of its own on a press; the page behind it
            // must not treat the same press as a click on the text around it.
            .relative()
            .child(self.block_probe(id))
            .child(self.render_table_grid(id, &source, &theme, cx))
            .into_any_element()
    }

    /// The table itself. A cell that is being edited holds a real field, so the
    /// text is changed where it is shown — no source listing above the grid.
    fn render_table_grid(
        &mut self,
        id: u64,
        source: &str,
        theme: &ThemeColor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(table) = model::parse_table(source) else {
            let at = id;
            return div()
                .w_full()
                .id(SharedString::from(format!("table-broken-{id}")))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        if let Some(index) = this.index_of(at) {
                            this.focus_block(index, window, cx);
                        }
                    }),
                )
                .child(
                    div()
                        .w_full()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .px_3()
                        .py_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("表格格式不完整：第二行需要是 | --- | 这样的分隔行"),
                )
                .into_any_element();
        };

        // The cell being typed in, if it belongs to this table.
        let editing = self.table_edit.as_ref().and_then(|edit| {
            (edit.block == id).then(|| (edit.row, edit.column, edit.state.clone()))
        });
        let columns = table.columns();

        let cell = |text: &str,
                    row: Option<usize>,
                    column: usize,
                    align: model::ColumnAlign,
                    header: bool,
                    cx: &mut Context<Self>|
         -> AnyElement {
            let mut frame = div()
                .flex_1()
                .min_w(px(56.))
                .border_r_1()
                .border_color(theme.border)
                .px_3()
                .py(px(4.))
                .text_size(px(TABLE_TEXT))
                .flex()
                .items_center();
            frame = match align {
                model::ColumnAlign::Left => frame.justify_start(),
                model::ColumnAlign::Center => frame.justify_center(),
                model::ColumnAlign::Right => frame.justify_end(),
            };
            if header {
                frame = frame
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground);
            }
            let cell = frame
                .id(SharedString::from(match row {
                    None => format!("cell-{id}-h-{column}"),
                    Some(row) => format!("cell-{id}-{row}-{column}"),
                }))
                .cursor_text()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.edit_table_cell(id, row, column, window, cx);
                        cx.stop_propagation();
                    }),
                );

            let open = editing
                .as_ref()
                .is_some_and(|(row_open, column_open, _)| {
                    *row_open == row && *column_open == column
                });
            if open {
                let state = editing.as_ref().map(|(_, _, state)| state.clone()).unwrap();
                // A one-line field, told the column's own alignment. A textarea
                // cannot take one — the engine only honours `text_align` on a
                // single-line field — and a centred cell that snaps to the left
                // the moment it is clicked reads as a jump.
                return cell
                    .child(
                        TextInput::new(&state)
                            .appearance(false)
                            .bordered(false)
                            .w_full()
                            .h(px(TABLE_TEXT * LINE_RATIO))
                            .p_0()
                            .text_size(px(TABLE_TEXT))
                            .line_height(relative(LINE_RATIO))
                            .text_color(theme.foreground)
                            .text_align(column_align(align)),
                    )
                    .into_any_element();
            }

            // Content-sized, so the cell's `justify_*` actually places it: a
            // `flex_1` child would fill the cell and read as left-aligned
            // always.
            let inline = inline::render(text);
            let empty = inline.text.is_empty();
            let table_chips: Vec<Range<usize>> = inline
                .spans
                .iter()
                .filter(|(_, flags)| flags & inline::CODE != 0)
                .map(|(range, _)| range.clone())
                .collect();
            let styled = with_code_face(
                styled_inline_chip(&inline.text, &inline.spans, theme),
                &table_chips,
            );
            let body = if empty {
                div()
                    .text_size(px(TABLE_TEXT))
                    .text_color(Hsla {
                        a: 0.4,
                        ..theme.muted_foreground
                    })
                    .child(" ")
                    .into_any_element()
            } else {
                styled.into_any_element()
            };
            cell.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .child(body)
                    .into_any_element(),
            )
            .into_any_element()
        };

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut header_cells: Vec<AnyElement> = Vec::new();
        for column in 0..columns {
            let align = table
                .align
                .get(column)
                .copied()
                .unwrap_or(model::ColumnAlign::Left);
            header_cells.push(cell(
                table.header.get(column).map(String::as_str).unwrap_or(""),
                None,
                column,
                align,
                true,
                cx,
            ));
        }
        rows.push(
            div()
                .flex()
                .w_full()
                .bg(theme.muted)
                .border_b_1()
                .border_color(theme.border)
                .children(header_cells)
                .into_any_element(),
        );

        for (row_index, row) in table.rows.iter().enumerate() {
            let mut cells: Vec<AnyElement> = Vec::new();
            for column in 0..columns {
                let align = table
                    .align
                    .get(column)
                    .copied()
                    .unwrap_or(model::ColumnAlign::Left);
                cells.push(cell(
                    row.get(column).map(String::as_str).unwrap_or(""),
                    Some(row_index),
                    column,
                    align,
                    false,
                    cx,
                ));
            }
            rows.push(
                div()
                    .flex()
                    .w_full()
                    .when(row_index % 2 == 1, |this| {
                        this.bg(Hsla {
                            a: 0.35,
                            ..theme.muted
                        })
                    })
                    .border_b_1()
                    .border_color(theme.border)
                    .children(cells)
                    .into_any_element(),
            );
        }

        // No "+ 添加一行" strip under the grid: rows are added where the caret
        // is (Ctrl+Enter) and removed the same way, so the table reads as a
        // table and not as a form with a button bolted to its bottom edge.

        div()
            .w_full()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .text_color(theme.foreground)
                    .children(rows),
            )
            .into_any_element()
    }

    /// Add an empty row right below the one being typed in, and keep the caret
    /// in the same column — a table grows where the work is, not only at its
    /// end, and a column of inputs stays a column.
    fn insert_table_row_below(
        &mut self,
        id: u64,
        row: Option<usize>,
        column: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let Some(mut table) = model::parse_table(&self.blocks[index].text) else {
            return;
        };
        self.remember(id);
        let columns = table.columns().max(1);
        // Under the header means above every body row.
        let at = match row {
            None => 0,
            Some(row) => (row + 1).min(table.rows.len()),
        };
        table.rows.insert(at, vec![String::new(); columns]);
        let markdown = table.to_markdown();
        self.blocks[index].text = markdown.clone().into();
        let state = self.blocks[index].state.clone();
        state.update(cx, |state, cx| state.set_value(markdown, window, cx));
        self.dirty = true;
        self.schedule_autosave(window, cx);
        self.edit_table_cell(id, Some(at), column, window, cx);
        cx.notify();
    }

    /// Drop the row the caret is typing in and stay in the table.
    ///
    /// The field has to go before the markdown changes: it is keyed by its
    /// coordinates, and the row that slides up into the deleted one's place
    /// would otherwise find a field still holding the old row's text and write
    /// it straight back.
    fn delete_table_row(
        &mut self,
        id: u64,
        row: Option<usize>,
        column: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The header is not a row that can go: it is what makes this a table.
        let Some(row) = row else {
            self.hint = "表头不能删除".into();
            cx.notify();
            return;
        };
        let Some(index) = self.index_of(id) else {
            return;
        };
        let Some(mut table) = model::parse_table(&self.blocks[index].text) else {
            return;
        };
        if row >= table.rows.len() {
            return;
        }
        self.remember(id);
        table.rows.remove(row);
        let markdown = table.to_markdown();
        self.blocks[index].text = markdown.clone().into();
        let state = self.blocks[index].state.clone();
        state.update(cx, |state, cx| state.set_value(markdown, window, cx));
        self.dirty = true;
        self.schedule_autosave(window, cx);
        self.table_edit = None;
        // The row that took its place, or the last one left when the last one
        // went. Never the header: it is not where the typing was.
        let next = if row < table.rows.len() {
            Some(row)
        } else {
            table.rows.len().checked_sub(1)
        };
        self.edit_table_cell(id, next, column, window, cx);
        cx.notify();
    }

    /// Ctrl+Enter from anywhere: rows belong to the cell the caret is in.
    fn add_table_row_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .table_edit
            .as_ref()
            .map(|edit| (edit.block, edit.row, edit.column));
        match target {
            Some((block, row, column)) => self.insert_table_row_below(block, row, column, window, cx),
            None => {
                self.hint = "先把光标放进表格的单元格里".into();
                cx.notify();
            }
        }
    }

    /// Ctrl+Shift+Backspace from anywhere.
    fn remove_table_row_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .table_edit
            .as_ref()
            .map(|edit| (edit.block, edit.row, edit.column));
        match target {
            Some((block, row, column)) => self.delete_table_row(block, row, column, window, cx),
            None => {
                self.hint = "先把光标放进表格的单元格里".into();
                cx.notify();
            }
        }
    }

    fn render_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().colors;
        let tab = self.settings_tab;
        let view = cx.entity();

        // A rail on the left, the page on the right. Every entry is a row of the
        // rail rather than a button in a strip, which is what makes the dialog
        // read like a settings page instead of a form.
        let nav_item = |cx: &mut Context<Self>,
                        label: &'static str,
                        value: SettingsTab,
                        id: &'static str|
         -> AnyElement {
            let selected = tab == value;
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .w_full()
                .h(px(36.))
                .px_3()
                .rounded(px(6.))
                .text_sm()
                .when(selected, |this| {
                    this.bg(theme.muted)
                        .text_color(theme.foreground)
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!selected, |this| {
                    this.text_color(theme.muted_foreground)
                        .hover(|style| style.bg(theme.muted))
                })
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.settings_tab = value;
                        cx.notify();
                    }),
                )
                .child(
                    // The accent bar marks the page you are on, the way a
                    // Fluent navigation view does.
                    div()
                        .flex_shrink_0()
                        .w(px(3.))
                        .h(px(16.))
                        .rounded_full()
                        .when(selected, |this| this.bg(theme.primary))
                        .when(!selected, |this| this.bg(transparent_black())),
                )
                .child(label)
                .into_any_element()
        };

        let body: AnyElement = match tab {
            SettingsTab::General => self.render_general_tab(cx),
            SettingsTab::Images => self.render_image_settings(cx),
            SettingsTab::Shortcuts => render_shortcut_table(cx),
        };

        let close_view = view.clone();
        div()
            .id("settings-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(Hsla { h: 0.0, s: 0.0, l: 0.0, a: 0.35 })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_settings(window, cx)),
            )
            // The wheel stops at the dialog as well. A press on the dim room
            // already closes the dialog instead of reaching the page; without
            // the same guard for the wheel, rolling it while reading a setting
            // walks the document behind, and closing the dialog leaves the
            // reader somewhere else entirely. Lists *inside* the dialog still
            // scroll: they sit deeper in the tree, so they see the wheel first.
            .on_scroll(|_: &ScrollEvent, _: &mut Window, cx: &mut App| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .id("settings-panel")
                    .flex()
                    .flex_col()
                    .w(px(880.))
                    .h(px(620.))
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .text_color(theme.foreground)
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_5()
                            .py_3()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(1.))
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("设置"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child("偏好会立刻生效，并保存在本地"),
                                    ),
                            )
                            .child(
                                Button::new("settings-close")
                                    .ghost()
                                    .small()
                                    .label("✕")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_settings(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .w(px(196.))
                                    .h_full()
                                    .flex_shrink_0()
                                    .p_3()
                                    .border_r_1()
                                    .border_color(theme.border)
                                    .bg(theme.sidebar)
                                    .child(nav_item(cx, "通用", SettingsTab::General, "tab-general"))
                                    .child(nav_item(cx, "图片与图床", SettingsTab::Images, "tab-images"))
                                    .child(nav_item(cx, "快捷键", SettingsTab::Shortcuts, "tab-keys")),
                            )
                            .child(
                                div()
                                    .id("settings-body")
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .min_h_0()
                                    .gap_4()
                                    .px_6()
                                    .py_5()
                                    .overflow_y_scroll()
                                    .child(body),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .px_5()
                            .py_3()
                            .border_t_1()
                            .border_color(theme.border)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(self.association.clone()),
                            )
                            .child(
                                Button::new("settings-done")
                                    .primary()
                                    .small()
                                    .label("完成")
                                    .on_click(move |_, window, cx| {
                                        close_view
                                            .update(cx, |this, cx| this.close_settings(window, cx));
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_general_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().colors;
        let is_dark = cx.theme().is_dark();
        let view = cx.entity();

        // One switch per row, with the switch pinned to the right edge so every
        // row in a group lines up.
        let switch_row = |title: &'static str,
                          hint: &'static str,
                          checked: bool,
                          id: &'static str,
                          apply: fn(&mut MeowDown, bool, &mut Context<MeowDown>)|
         -> AnyElement {
            let view = view.clone();
            setting_row(
                title,
                hint,
                Switch::new(id)
                    .checked(checked)
                    .on_click(move |value, _window, cx: &mut App| {
                        let value = *value;
                        view.update(cx, |this, cx| {
                            apply(this, value, cx);
                            cx.notify();
                        });
                    })
                    .into_any_element(),
                &theme,
            )
        };

        let segment = |cx: &mut Context<Self>,
                       label: &'static str,
                       selected: bool,
                       id: &'static str,
                       value: SegmentValue|
         -> AnyElement {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .h(px(28.))
                .px_4()
                .rounded(px(5.))
                .text_sm()
                .cursor_pointer()
                .when(selected, |this| {
                    this.bg(theme.background)
                        .text_color(theme.foreground)
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!selected, |this| {
                    this.text_color(theme.muted_foreground)
                        .hover(|style| style.text_color(theme.foreground))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| match value {
                        SegmentValue::Light => {
                            Theme::change(ThemeMode::Light, Some(window), cx);
                            this.hint = "浅色主题".into();
                            cx.notify();
                        }
                        SegmentValue::Dark => {
                            Theme::change(ThemeMode::Dark, Some(window), cx);
                            this.hint = "深色主题".into();
                            cx.notify();
                        }
                        SegmentValue::Relative | SegmentValue::Absolute => {
                            let mode = if value == SegmentValue::Absolute {
                                ImageMode::Absolute
                            } else {
                                ImageMode::Relative
                            };
                            this.settings.image_mode = mode;
                            cx.notify();
                        }
                    }),
                )
                .child(label)
                .into_any_element()
        };

        let current_mode = self.settings.image_mode;
        let theme_control = div()
            .flex()
            .items_center()
            .gap(px(1.))
            .p(px(2.))
            .rounded(px(7.))
            .bg(theme.muted)
            .child(segment(cx, "浅色", !is_dark, "theme-light", SegmentValue::Light))
            .child(segment(cx, "深色", is_dark, "theme-dark", SegmentValue::Dark))
            .into_any_element();

        let mode_control = div()
            .flex()
            .items_center()
            .gap(px(1.))
            .p(px(2.))
            .rounded(px(7.))
            .bg(theme.muted)
            .child(segment(
                cx,
                ImageMode::Relative.label(),
                current_mode == ImageMode::Relative,
                "mode-relative",
                SegmentValue::Relative,
            ))
            .child(segment(
                cx,
                ImageMode::Absolute.label(),
                current_mode == ImageMode::Absolute,
                "mode-absolute",
                SegmentValue::Absolute,
            ))
            .into_any_element();

        let auto_save = self.settings.auto_save;
        let auto_view = view.clone();
        let auto_control = Switch::new("switch-autosave")
            .checked(auto_save)
            .on_click(move |value, window, cx: &mut App| {
                let value = *value;
                auto_view.update(cx, |this, cx| {
                    this.settings.auto_save = value;
                    if let Err(err) = this.settings.save() {
                        this.hint = format!("设置保存失败：{err}").into();
                    } else if value {
                        this.schedule_autosave(window, cx);
                        this.hint = "自动保存已开启 · 停手一秒后写回磁盘".into();
                    } else {
                        this.hint = "自动保存已关闭".into();
                    }
                    cx.notify();
                });
            })
            .into_any_element();

        let assoc_status = assoc::status();
        let assoc_control = div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("当前状态：{assoc_status}")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("assoc-register")
                            .small()
                            .primary()
                            .label("设为 .md 默认应用")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.association = match std::env::current_exe()
                                    .map_err(|err| err.to_string())
                                    .and_then(|exe| assoc::register(&exe))
                                {
                                    Ok(()) => {
                                        "已写入注册表。若 Windows 仍用别的程序打开，请在「设置 → 应用 → 默认应用」里选择 MeowDown。"
                                            .into()
                                    }
                                    Err(err) => format!("关联失败：{err}").into(),
                                };
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("assoc-remove")
                            .small()
                            .ghost()
                            .label("取消关联")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.association = match assoc::unregister() {
                                    Ok(()) => "已移除 MeowDown 的 .md 关联".into(),
                                    Err(err) => format!("取消失败：{err}").into(),
                                };
                                cx.notify();
                            })),
                    )
                    .child(
                        // Windows 10 起 Explorer 只认 `UserChoice`，而那个键
                        // 只允许用户在设置里改。程序能做的最后一件事，是把
                        // 用户送到那一页，而不是让他自己在设置里翻。
                        Button::new("assoc-settings")
                            .small()
                            .ghost()
                            .label("去系统设置")
                            .on_click(cx.listener(|this, _, _, cx| {
                                assoc::open_default_apps();
                                this.association =
                                    "已打开 Windows 的默认应用设置，在列表里选 MeowDown".into();
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element();

        // The sample shows itself once and then never again, so there has to
        // be a door back to it: a reader who closed it before reading the
        // shortcut list would otherwise have no way of finding it.
        let sample_control = div()
            .flex()
            .items_center()
            .child(
                Button::new("open-sample")
                    .small()
                    .ghost()
                    .label("查看示例文档")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if !this.settle_before_switch(window, cx) {
                            return;
                        }
                        this.set_document(SAMPLE, None, window, cx);
                        this.sample = true;
                        this.close_settings(window, cx);
                        this.hint = "示例文档 · 直接开始编辑，Ctrl+S 保存".into();
                        cx.notify();
                    })),
            )
            .into_any_element();

        let mut column = div().flex().flex_col().gap_5().w_full();

        column = column.child(setting_group(
            "外观",
            &theme,
            vec![
                setting_row("主题", "浅色适合白天，深色适合夜里写东西", theme_control, &theme),
                switch_row(
                    "显示侧边栏",
                    "Ctrl+Shift+L 也可以切换",
                    self.sidebar_visible,
                    "switch-sidebar",
                    |this, value, cx| this.set_sidebar_open(value, cx),
                ),
                switch_row(
                    "专注模式",
                    "只突出当前正在编辑的段落（F8）",
                    self.focus_mode,
                    "switch-focus",
                    |this, value, _| this.focus_mode = value,
                ),
            ],
        ));

        column = column.child(setting_group(
            "编辑与保存",
            &theme,
            vec![
                setting_row(
                    "自动保存",
                    "停手一秒后自动写回磁盘，不必再按 Ctrl+S",
                    auto_control,
                    &theme,
                ),
                setting_field(
                    "插入图片时的路径",
                    "复制到 assets 会把图片存到文档旁边，写相对路径；整个文件夹拷走也不会丢图。",
                    mode_control,
                    &theme,
                ),
                setting_field(
                    "assets 目录名",
                    "图片复制到文档旁边的哪个文件夹",
                    field_row_inline(&self.fields.assets_dir, &theme),
                    &theme,
                ),
            ],
        ));

        column = column.child(setting_group(
            "系统集成",
            &theme,
            vec![
                setting_field(
                    "文件关联",
                    "让双击 .md 文件时用 MeowDown 打开",
                    assoc_control,
                    &theme,
                ),
                setting_field(
                    "示例文档",
                    "第一次打开程序时自动显示一次，之后可以在这里再看",
                    sample_control,
                    &theme,
                ),
            ],
        ));

        column.into_any_element()
    }

    fn render_image_settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().colors;
        let host = self.settings.host;
        let view = cx.entity();

        let host_button = |cx: &mut Context<Self>, value: ImageHost, id: &'static str| -> AnyElement {
            let current = cx.entity();
            let selected = host == value;
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .h(px(28.))
                .px_4()
                .rounded(px(5.))
                .text_sm()
                .cursor_pointer()
                .when(selected, |this| {
                    this.bg(theme.background)
                        .text_color(theme.foreground)
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!selected, |this| {
                    this.text_color(theme.muted_foreground)
                        .hover(|style| style.text_color(theme.foreground))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    move |_, _, cx: &mut App| {
                        current.update(cx, |this, cx| {
                            this.settings.host = value;
                            this.settings.apply_host_defaults();
                            cx.notify();
                        });
                    },
                )
                .child(value.label())
                .into_any_element()
        };

        let auto_view = view.clone();
        let auto_upload = self.settings.auto_upload;
        let upload_switch = Switch::new("switch-upload")
            .checked(auto_upload)
            .on_click(move |value, _window, cx: &mut App| {
                let value = *value;
                auto_view.update(cx, |this, cx| {
                    this.settings.auto_upload = value;
                    cx.notify();
                });
            })
            .into_any_element();

        let host_control = div()
            .flex()
            .items_center()
            .gap(px(1.))
            .p(px(2.))
            .rounded(px(7.))
            .bg(theme.muted)
            .child(host_button(cx, ImageHost::PicGo, "host-picgo"))
            .child(host_button(cx, ImageHost::SmMs, "host-smms"))
            .child(host_button(cx, ImageHost::Custom, "host-custom"))
            .into_any_element();

        let mut connection: Vec<AnyElement> = Vec::new();
        connection.push(setting_field(
            "上传地址",
            "图片会以 multipart 表单 POST 到这里",
            match host {
                ImageHost::Custom => field_row_inline(&self.fields.endpoint, &theme),
                _ => div()
                    .text_xs()
                    .font_family(mono_font())
                    .text_color(theme.foreground)
                    .child(self.settings.endpoint.clone())
                    .into_any_element(),
            },
            &theme,
        ));
        if host != ImageHost::PicGo {
            connection.push(setting_field(
                "API Token",
                "可填 Authorization 的原值",
                field_row_inline(&self.fields.token, &theme),
                &theme,
            ));
        }
        match host {
            ImageHost::Custom => {
                connection.push(setting_field(
                    "表单字段名",
                    "上传时图片放在哪个字段里，常见是 image 或 file",
                    field_row_inline(&self.fields.field, &theme),
                    &theme,
                ));
                connection.push(setting_field(
                    "响应里图片地址的路径",
                    "例如 data.url 或 result[0]",
                    field_row_inline(&self.fields.url_path, &theme),
                    &theme,
                ));
            }
            ImageHost::SmMs => {
                connection.push(setting_field(
                    "固定约定",
                    "表单字段 smfile · 响应路径 data.url",
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("不需要填写")
                        .into_any_element(),
                    &theme,
                ));
            }
            ImageHost::PicGo => {
                connection.push(setting_field(
                    "固定约定",
                    "会向 PicGo 发送 {\"list\": [图片绝对路径]}，读取 result[0]",
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("不需要填写")
                        .into_any_element(),
                    &theme,
                ));
            }
        }

        let mut column = div().flex().flex_col().gap_5().w_full();

        // PicGo lives on the user's disk and has to be running for an upload to
        // work, so its location is part of the settings. It is looked for
        // automatically; this is where a copy that went somewhere unusual is
        // pointed at.
        if host == ImageHost::PicGo {
            let found = settings::detect_picgo(&self.settings.picgo_path);
            let saved = !self.settings.picgo_path.trim().is_empty();
            let label = match &found {
                Some(path) => path.to_string_lossy().to_string(),
                None => "没有找到 PicGo，上传时会提示指定位置".to_string(),
            };
            let action = if found.is_some() { "修改位置…" } else { "选择位置…" };
            let clear = {
                let view = cx.entity();
                div()
                    .id("picgo-clear")
                    .flex()
                    .items_center()
                    .h(px(26.))
                    .px_3()
                    .rounded(px(5.))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.foreground))
                    .when(!saved, |this| this.opacity(0.35))
                    .on_mouse_down(MouseButton::Left, move |_, _, cx: &mut App| {
                        view.update(cx, |this, cx| {
                            this.settings.picgo_path.clear();
                            let _ = this.settings.save();
                            this.hint = "已清除 PicGo 位置，下次上传会重新检测".into();
                            cx.notify();
                        });
                    })
                    .child("自动检测")
                    .into_any_element()
            };
            column = column.child(setting_group(
                "PicGo",
                &theme,
                vec![
                    setting_field(
                        "程序位置",
                        "上传时会先检测这个程序；没在运行就自动启动它",
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_xs()
                                    .font_family(mono_font())
                                    .text_color(if found.is_some() {
                                        theme.foreground
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .child(label),
                            )
                            .child(
                                Button::new("picgo-pick")
                                    .small()
                                    .label(action)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.pick_picgo(window, cx)
                                    })),
                            )
                            .child(clear)
                            .into_any_element(),
                        &theme,
                    ),
                    setting_row(
                        "自动启动",
                        "上传时如果 PicGo 没打开，会先启动它并等它就绪，再上传",
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("已开启")
                            .into_any_element(),
                        &theme,
                    ),
                ],
            ));
        }

        column = column.child(setting_group(
            "上传",
            &theme,
            vec![setting_row(
                "插入图片时自动上传",
                "上传成功后，文档里的路径会替换成图床返回的网络地址",
                upload_switch,
                &theme,
            )],
        ));

        column = column.child(setting_group(
            "图床服务",
            &theme,
            vec![
                setting_field("服务商", host.hint(), host_control, &theme),
            ],
        ));

        column = column.child(setting_group("连接信息", &theme, connection));

        column = column.child(setting_group(
            "操作",
            &theme,
            vec![setting_field(
                "批量上传与测试",
                "把本文档里还在用本地路径的图片一次性传上去，或者先试一张",
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("upload-all")
                            .small()
                            .label("上传本文档全部本地图片")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.upload_all_images(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("test-upload")
                            .small()
                            .ghost()
                            .label("测试上传一张图")
                            .on_click(cx.listener(|this, _, window, cx| this.test_upload(window, cx))),
                    )
                    .into_any_element(),
                &theme,
            )],
        ));

        column.into_any_element()
    }

    fn render_status(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().colors;
        let word_count = model::count_words(&self.document_text());
        let blocks = self.blocks.len();
        let hint = self.hint.clone();
        let path = self
            .path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| "尚未保存到磁盘".to_string());

        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .h(px(30.))
            .px_4()
            .flex_shrink_0()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(div().overflow_hidden().text_ellipsis().child(path))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(hint)
                    .child(format!("{blocks} 段"))
                    .child(format!("{word_count} 字"))
                    .child(if self.dirty {
                        "未保存"
                    } else if self.path.is_some() {
                        "已保存"
                    } else {
                        "新文档"
                    }),
            )
            .into_any_element()
    }
}

/// What a two-way segment in the settings page stands for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SegmentValue {
    Light,
    Dark,
    Relative,
    Absolute,
}

/// A Fluent-style group: a quiet heading over a card of rows, with hairline
/// dividers between them.
fn setting_group(title: &'static str, theme: &ThemeColor, rows: Vec<AnyElement>) -> AnyElement {
    let mut children: Vec<AnyElement> = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            children.push(
                div()
                    .h(px(1.))
                    .w_full()
                    .bg(theme.secondary)
                    .into_any_element(),
            );
        }
        children.push(row);
    }

    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .child(
            div()
                .pl_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(title),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .rounded(px(8.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.muted)
                .overflow_hidden()
                .children(children),
        )
        .into_any_element()
}

/// One row of a group: what it does on the left, the control on the right.
fn setting_row(
    title: &'static str,
    hint: &'static str,
    control: AnyElement,
    theme: &ThemeColor,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_6()
        .w_full()
        .px_4()
        .py(px(12.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .child(div().text_sm().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(hint),
                ),
        )
        .child(div().flex_shrink_0().child(control))
        .into_any_element()
}

/// A row whose control needs the full width: a text field, a status line, a
/// pair of buttons.
fn setting_field(
    title: &'static str,
    hint: &'static str,
    control: AnyElement,
    theme: &ThemeColor,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .px_4()
        .py(px(12.))
        .child(div().text_sm().child(title))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(hint),
        )
        .child(control)
        .into_any_element()
}

/// A bordered input the width of the row, without the label `field_row` adds.
fn field_row_inline(state: &Entity<TextareaState>, theme: &ThemeColor) -> AnyElement {
    div()
        .w_full()
        .px_2()
        .py(px(6.))
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(
            Textarea::new(state)
                .appearance(false)
                .bordered(false)
                .w_full()
                .p_0()
                .font_family(mono_font())
                .text_size(px(13.))
                .text_color(theme.foreground),
        )
        .into_any_element()
}

fn render_shortcut_table(cx: &App) -> AnyElement {
    let theme = cx.theme().colors;
    let mut groups: Vec<AnyElement> = Vec::new();
    for (title, items) in SHORTCUTS.iter() {
        let mut rows: Vec<AnyElement> = Vec::new();
        for shortcut in items.iter() {
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .w_full()
                    .px_4()
                    .py(px(8.))
                    .child(div().text_sm().child(shortcut.action))
                    .child(
                        div()
                            .flex_shrink_0()
                            .px_2()
                            .py(px(2.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .font_family(mono_font())
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(shortcut.keys),
                    )
                    .into_any_element(),
            );
        }
        groups.push(setting_group(title, &theme, rows));
    }
    div()
        .flex()
        .flex_col()
        .gap_5()
        .w_full()
        .children(groups)
        .into_any_element()
}

impl Render for MeowDown {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().colors;
        // The title bar's ✕ belongs to the platform, which knows nothing about
        // unsaved work. The hook is installed on the first frame — the first
        // moment this view has a handle of its own — and answers for the
        // window: an empty page goes quietly, anything typed into holds it
        // back so the question can be put on screen.
        if !self.close_hooked {
            self.close_hooked = true;
            let view = cx.entity();
            window.on_window_should_close(cx, move |window, cx| {
                view.update(cx, |this, cx| this.before_close(window, cx))
            });
        }
        // The panel's width is resolved first: the fold is advanced here, so
        // the panel and the text beside it are laid out at the same width in
        // the same frame, and the text widens as the panel gives way.
        let sidebar_width = self.sidebar_width(window);
        let sidebar = if sidebar_width > 0.5 {
            // The panel is built at its full width and the box around it is
            // what narrows: the lists inside keep their layout and are simply
            // clipped by the edge, which reads as the panel sliding away
            // instead of its contents squeezing into a sliver.
            Some(
                div()
                    .w(px(sidebar_width))
                    .h_full()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .border_r_1()
                    .border_color(theme.sidebar_border)
                    .child(self.render_sidebar(window, cx))
                    .into_any_element(),
            )
        } else {
            None
        };
        let body = self.render_body(window, sidebar_width, cx);
        let settings = if self.show_settings {
            Some(self.render_settings(window, cx))
        } else {
            None
        };
        let menu = if self.menu.is_some() {
            Some(self.render_menu(window, cx))
        } else {
            None
        };
        let prompt = if self.prompt.is_some() {
            Some(self.render_prompt(window, cx))
        } else {
            None
        };
        let lang_menu = if self.lang_menu.is_some() {
            Some(self.render_lang_menu(window, cx))
        } else {
            None
        };
        let viewer = if self.viewer.is_some() {
            Some(self.render_viewer(window, cx))
        } else {
            None
        };
        let close_prompt = if self.close_prompt {
            Some(self.render_close_prompt(cx))
        } else {
            None
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .font_family(ui_font())
            .bg(theme.background)
            .text_color(theme.foreground)
            // Capture phase: the focused text field would otherwise eat
            // Ctrl+V, Ctrl+B, the arrow keys and friends before the document
            // ever sees them.
            .capture_key_down(cx.listener(Self::on_key_down))
            // Keys that the field binds to an action never reach the listener
            // above, so the ones the document cares about are intercepted as
            // actions in the capture phase instead.
            .capture_action(cx.listener(Self::on_paste))
            .capture_action(cx.listener(Self::on_undo))
            .capture_action(cx.listener(Self::on_redo))
            .capture_action(cx.listener(Self::on_move_up))
            .capture_action(cx.listener(Self::on_move_down))
            .capture_action(cx.listener(Self::on_move_left))
            .capture_action(cx.listener(Self::on_move_right))
            .child(self.render_menubar(window, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .children(sidebar)
                    .child(body),
            )
            .child(self.render_status(window, cx))
            .children(settings)
            .children(menu)
            .children(lang_menu)
            .children(prompt)
            .children(viewer)
            .children(close_prompt)
    }
}

#[cfg(test)]
mod tests {
    // Imported by name on purpose: a glob would also pull GPUI's `#[test]`
    // attribute macro into scope and it does not apply here.
    use super::{
        chip_content, chip_parts, copy_path_for, file_name, folder_tree, is_markdown, lang_matches,
        line_runs, markdown_files, mono_font, name_problem, point, px,
        reading_column, relative_label, resolve_address, search_hits, size, strip_marks, unused_path,
        viewer_bounds, viewer_fit, wrapped_index, zoomed_offset, ScrollAnim, Slide, PAGE_MIN_WIDTH,
        PAGE_WIDTH_RATIO, SIDEBAR_SLIDE, SIDEBAR_WIDTH, VIEWER_PAD,
    };
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// Copying prose out of a chip should not carry the backticks that hold the
    /// chip open — they are furniture, not words.
    #[test]
    fn a_copied_slice_leaves_the_chip_markers_behind() {
        // "用 `代码` 算" — the backticks sit at bytes 4 and 11.
        let text = "用 `代码` 算";
        let marks = vec![4..5, 11..12];
        assert_eq!(strip_marks(text, 0, &marks), "用 代码 算");
        // A slice that starts after the opening marker still loses the closing
        // one.
        assert_eq!(strip_marks(&text[5..], 5, &marks), "代码 算");
        // A copy that starts on a marker drops it, and keeps the rest.
        assert_eq!(strip_marks(&text[11..], 11, &marks), " 算");
    }

    /// A chip's text is code and is set in the code face; the two blanks that
    /// pad it are prose and stay in the prose's face. If the blanks changed
    /// face too, every chip would be wider than the one beside it.
    #[test]
    fn a_line_is_cut_into_prose_and_code() {
        let inline = crate::inline::render("用 `a * b` 算");
        let chips: Vec<std::ops::Range<usize>> = inline
            .spans
            .iter()
            .filter(|(_, flags)| flags & crate::inline::CODE != 0)
            .map(|(range, _)| range.clone())
            .collect();
        assert_eq!(chips.len(), 1);
        assert_eq!(&inline.text[chip_content(&chips[0]).unwrap()], "a * b");

        let runs = line_runs(&inline.text, false, &chips);
        let total: usize = runs.iter().map(|run| run.len).sum();
        assert_eq!(total, inline.text.len(), "the runs have to cover the line");

        let mut at = 0;
        let mut cut: Vec<(&str, gpui_kit::SharedString)> = Vec::new();
        for run in &runs {
            cut.push((&inline.text[at..at + run.len], run.font.family.clone()));
            at += run.len;
        }
        assert_eq!(cut[1].0, "a * b");
        assert_eq!(cut[1].1, gpui_kit::SharedString::from(mono_font()));
        assert_ne!(cut[0].1, gpui_kit::SharedString::from(mono_font()));
    }

    /// A chip has to reach the shaper as three runs, not one: the family
    /// override that puts its word in the code face only lands on runs it
    /// covers whole, and a single run for the whole chip sticks out past it on
    /// both ends. One run there means prose glyphs inside a pill measured in
    /// the code face — the word sits left and the right of the pill runs over
    /// whatever follows it.
    #[test]
    fn a_chip_is_cut_into_blank_word_blank() {
        let inline = crate::inline::render("用 `a * b` 算");
        let chip = inline.spans.first().expect("one code span").0.clone();
        let parts = chip_parts(&chip).expect("a chip has a middle");
        assert_eq!(&inline.text[parts[0].clone()], " ", "the leading blank");
        assert_eq!(&inline.text[parts[1].clone()], "a * b", "the word");
        assert_eq!(&inline.text[parts[2].clone()], " ", "the trailing blank");
        assert_eq!(parts[0].start, chip.start);
        assert_eq!(parts[2].end, chip.end);
    }

    #[test]
    fn addresses_resolve_schemes_files_and_bare_hosts() {
        let dir = std::env::temp_dir().join(format!("meowdown-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("说明.md"), "# x").unwrap();

        // An address with a scheme is handed over untouched.
        assert_eq!(
            resolve_address("https://gpui.rs", &dir).as_deref(),
            Some("https://gpui.rs")
        );
        assert_eq!(
            resolve_address("mailto:a@b.c", &dir).as_deref(),
            Some("mailto:a@b.c")
        );
        // A bare host gets its scheme back, otherwise the shell reads it as a
        // file name.
        assert_eq!(
            resolve_address("www.example.com/x", &dir).as_deref(),
            Some("https://www.example.com/x")
        );
        // A relative path is resolved against the document's folder.
        let local = resolve_address("说明.md", &dir).expect("local file");
        assert!(Path::new(&local).is_file());
        // An anchor inside the document has nothing for the shell to open, and
        // a path that does not exist is a miss rather than a broken launch.
        assert_eq!(resolve_address("#一段标题", &dir), None);
        assert_eq!(resolve_address("没有这个文件.md", &dir), None);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_listing_keeps_only_markdown_and_sorts_by_name() {
        let dir = std::env::temp_dir().join(format!("meowdown-listing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        for name in ["b.md", "A.MD", "c.markdown", "note.txt"] {
            std::fs::write(dir.join(name), "# x").unwrap();
        }
        std::fs::write(dir.join("nested/deep.md"), "# deep").unwrap();

        let files = markdown_files(&dir);
        let names: Vec<String> = files.iter().map(|path| file_name(path)).collect();
        assert_eq!(names, vec!["A.MD", "b.md", "c.markdown"]);
        // The listing is flat: subfolders stay out of it.
        assert!(files.iter().all(|path| path.parent() == Some(dir.as_path())));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn markdown_extension_check_ignores_case() {
        assert!(is_markdown(Path::new("a.md")));
        assert!(is_markdown(Path::new("a.MARKDOWN")));
        assert!(!is_markdown(Path::new("a.txt")));
        assert!(!is_markdown(Path::new("a")));
    }

    /// Opening a folder reads its top level and nothing below it: a subfolder
    /// is read when it is opened, which is what keeps a crowded parent folder
    /// from being walked in full before the first frame.
    #[test]
    fn the_tree_reads_one_level_at_a_time() {
        let dir = std::env::temp_dir().join(format!("meowdown-deep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("notes/2026")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        std::fs::write(dir.join("root.md"), "# root").unwrap();
        std::fs::write(dir.join("notes/2026/一月.md"), "# 一月").unwrap();
        std::fs::write(dir.join("notes/notes.md"), "# notes").unwrap();
        std::fs::write(dir.join(".git/x.md"), "# hidden").unwrap();
        std::fs::write(dir.join("node_modules/pkg/y.md"), "# vendor").unwrap();
        std::fs::write(dir.join("notes/skip.txt"), "no").unwrap();

        let mut tree = folder_tree(&dir);
        // The root is read: its own Markdown, and a row per folder worth
        // opening. Dot-folders and build output stay out.
        assert_eq!(tree.file_count(), Some(1));
        let names: Vec<&str> = tree.dirs.iter().map(|child| child.name.as_str()).collect();
        assert_eq!(names, vec!["notes"]);
        // The folder row is a stub: nothing below the root has been read, so it
        // can neither be counted nor called empty.
        assert_eq!(tree.dirs[0].file_count(), None);
        assert!(!tree.dirs[0].shows_nothing());

        // Opening `notes` — and, on the way, `notes/2026` — reads those levels.
        tree.load_path(&dir.join("notes/2026"));
        assert_eq!(tree.dirs[0].file_count(), Some(1));
        let inner: Vec<&str> = tree.dirs[0]
            .dirs
            .iter()
            .map(|child| child.name.as_str())
            .collect();
        assert_eq!(inner, vec!["2026"]);
        assert_eq!(tree.dirs[0].dirs[0].file_count(), Some(1));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A folder the user made should be in the tree even when it holds nothing
    /// this panel can open: an empty branch that vanished reads as a bug, and
    /// so does an `img/` full of screenshots.
    #[test]
    fn the_tree_keeps_folders_that_hold_no_markdown() {
        let dir = std::env::temp_dir().join(format!("meowdown-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        std::fs::create_dir_all(dir.join("img/deep")).unwrap();
        std::fs::create_dir_all(dir.join("notes/2026")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("root.md"), "# root").unwrap();
        std::fs::write(dir.join("notes/2026/一月.md"), "# 一月").unwrap();
        std::fs::write(dir.join("img/shot.png"), "png").unwrap();

        let mut tree = folder_tree(&dir);
        let names: Vec<&str> = tree.dirs.iter().map(|child| child.name.as_str()).collect();
        // Sorted, folders that hold nothing included, dot-folders left out.
        assert_eq!(names, vec!["empty", "img", "notes"]);
        assert_eq!(tree.file_count(), Some(1));
        // Reading a folder that holds nothing shows it as empty; one that has
        // only a subfolder keeps a way in.
        let empty = tree.dirs.iter_mut().find(|child| child.name == "empty").unwrap();
        empty.load();
        assert!(empty.shows_nothing());
        let img = tree.dirs.iter_mut().find(|child| child.name == "img").unwrap();
        img.load();
        assert_eq!(img.file_count(), Some(0));
        assert!(!img.shows_nothing());
        // `img/deep` is empty too, so it is a leaf with no way to open it.
        img.dirs[0].load();
        assert!(img.dirs[0].shows_nothing());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A new file or folder must not land on a name that is already there.
    #[test]
    fn a_new_name_steps_around_what_is_already_there() {
        let dir = std::env::temp_dir().join(format!("meowdown-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let first = unused_path(&dir, "新建文档", Some("md"));
        assert_eq!(file_name(&first), "新建文档.md");
        std::fs::write(&first, "").unwrap();
        assert_eq!(file_name(&unused_path(&dir, "新建文档", Some("md"))), "新建文档 2.md");

        // A folder has no extension to keep.
        let folder = unused_path(&dir, "新建文件夹", None);
        assert_eq!(file_name(&folder), "新建文件夹");
        std::fs::create_dir(&folder).unwrap();
        assert_eq!(
            file_name(&unused_path(&dir, "新建文件夹", None)),
            "新建文件夹 2"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A duplicate says it is one, and keeps the extension where it belongs.
    #[test]
    fn a_copy_says_so_in_its_name() {
        let dir = std::env::temp_dir().join(format!("meowdown-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("笔记.md");
        std::fs::write(&path, "# 一").unwrap();

        let first = copy_path_for(&path);
        assert_eq!(file_name(&first), "笔记 - 副本.md");
        std::fs::write(&first, "# 一").unwrap();
        assert_eq!(file_name(&copy_path_for(&path)), "笔记 - 副本 2.md");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Windows refuses a handful of characters in a name. The prompt says which
    /// one instead of letting `rename` fail with an opaque error.
    #[test]
    fn a_name_windows_would_refuse_is_refused_here() {
        assert!(name_problem("").is_some());
        assert!(name_problem("a/b").is_some());
        assert!(name_problem("a:b").is_some());
        assert!(name_problem("a?b").is_some());
        assert!(name_problem("..").is_some());
        assert!(name_problem("尾巴.").is_some());
        assert!(name_problem("尾巴 ").is_some());
        assert!(name_problem("笔记 2026-11-08.md").is_none());
    }

    /// The search box finds files by name, at any depth, and lists them by
    /// their path inside the folder so two `笔记.md` can be told apart.
    #[test]
    fn a_search_finds_names_at_any_depth() {
        let dir = std::env::temp_dir().join(format!("meowdown-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("会议")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("会议记录.md"), "# 一").unwrap();
        std::fs::write(dir.join("会议/周会.md"), "# 二").unwrap();
        std::fs::write(dir.join("其他.md"), "# 三").unwrap();
        std::fs::write(dir.join(".git/会议.md"), "# 不该出现").unwrap();
        std::fs::write(dir.join("会议/图.png"), "png").unwrap();

        let labels: Vec<String> = search_hits(&dir, "会议")
            .iter()
            .map(|path| relative_label(&dir, path))
            .collect();
        assert_eq!(labels, vec!["会议/周会.md", "会议记录.md"]);
        // Nothing typed, nothing found.
        assert!(search_hits(&dir, "  ").is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The reading column keeps a margin on both sides, follows the window
    /// while it can, and stops growing once the measure is comfortable.
    #[test]
    fn the_reading_column_takes_95_percent_and_keeps_growing() {
        // The column is 95% of the content area, whatever the window is; the
        // other 5% is split evenly into the two margins.
        let column = reading_column(px(1000.), SIDEBAR_WIDTH);
        let available = px(1000. - SIDEBAR_WIDTH);
        assert_eq!(column, available * PAGE_WIDTH_RATIO);
        // The share survives the round trip through pixels.
        assert!((column.as_f32() / available.as_f32() - PAGE_WIDTH_RATIO).abs() < 0.0005);
        // Wider window, wider column — there is no cap to run into.
        assert!(reading_column(px(2400.), SIDEBAR_WIDTH) > reading_column(px(1000.), SIDEBAR_WIDTH));
        assert_eq!(
            reading_column(px(2400.), SIDEBAR_WIDTH),
            px(2400. - SIDEBAR_WIDTH) * PAGE_WIDTH_RATIO
        );
        // A panel toggled off hands its width to the text.
        assert!(reading_column(px(1000.), 0.) > column);
        // Narrow: the margins give way before the text does, down to a point,
        // and past that the window is simply too small for both.
        assert_eq!(reading_column(px(500.), 0.), px(500.) * PAGE_WIDTH_RATIO);
        assert_eq!(reading_column(px(300.), 0.), px(PAGE_MIN_WIDTH));
    }

    #[test]
    fn the_side_panel_folds_away_over_a_short_glide() {
        let start = Instant::now();
        let fold = Slide {
            from: 1.,
            to: 0.,
            start,
            duration: SIDEBAR_SLIDE,
        };
        // It starts where the panel is, and lands exactly folded.
        assert_eq!(fold.value_at(start), (1., false));
        let (end, done) = fold.value_at(start + SIDEBAR_SLIDE + Duration::from_millis(10));
        assert_eq!(end, 0.);
        assert!(done);

        // The width it hands to the layout walks down without ever overshooting
        // either end, so the panel cannot flash wider than it opens.
        let mut previous = SIDEBAR_WIDTH;
        for step in 1..=12 {
            let at = start + Duration::from_millis(step * 16);
            let (value, _) = fold.value_at(at);
            let width = SIDEBAR_WIDTH * value;
            assert!(width <= previous, "the fold must only ever narrow");
            assert!((0. ..=SIDEBAR_WIDTH).contains(&width));
            previous = width;
        }
        // Halfway through the time it is already most of the way gone — the
        // ease-out is what makes a 190ms fold read as instant but not abrupt.
        let (half, _) = fold.value_at(start + Duration::from_millis(95));
        assert!(half < 0.5);

        // The text beside it widens as the panel gives way: a half-folded panel
        // leaves half its width to the column.
        assert!(
            reading_column(px(1000.), SIDEBAR_WIDTH / 2.)
                > reading_column(px(1000.), SIDEBAR_WIDTH)
        );
    }

    #[test]
    fn scroll_animation_eases_out_and_arrives() {
        let start = Instant::now();
        let anim = ScrollAnim {
            from: px(0.),
            to: px(-400.),
            start,
            duration: Duration::from_millis(260),
        };
        // Nothing moves before it starts, and it ends exactly on target.
        assert_eq!(anim.offset_at(start).0, px(0.));
        let (end, done) = anim.offset_at(start + Duration::from_millis(400));
        assert_eq!(end, px(-400.));
        assert!(done);

        // Monotonic, and ahead of a straight line: that is the ease-out part.
        let mut previous = px(0.);
        for step in 1..=10 {
            let (now, done) = anim.offset_at(start + Duration::from_millis(step * 26));
            assert!(now <= previous);
            assert!(!done || step == 10);
            previous = now;
        }
        let (half, _) = anim.offset_at(start + Duration::from_millis(130));
        assert!(half <= px(-200.), "ease-out should pass halfway early");
        assert!(half > px(-400.));
    }

    #[test]
    fn a_picture_is_fitted_to_the_window_and_never_blown_up() {
        let window = size(px(1000.), px(700.));
        // A big picture shrinks until the tighter of the two axes fits, with
        // the padding kept clear on both sides.
        let fit = viewer_fit(size(1200.0, 800.0), window);
        let room_w = 1000.0 - VIEWER_PAD * 2.0;
        let room_h = 700.0 - VIEWER_PAD * 2.0;
        assert!((fit - (room_w / 1200.0).min(room_h / 800.0)).abs() < 1e-6);
        assert!(fit < 1.0);

        // A small picture is shown at its own size: upscaling a 64px icon to
        // fill the window only makes it blurry.
        assert_eq!(viewer_fit(size(64.0, 64.0), window), 1.0);
    }

    #[test]
    fn the_picture_sits_in_the_middle_until_it_is_dragged() {
        let window = size(px(1000.), px(700.));
        let image = size(400.0, 300.0);
        let centered = viewer_bounds(image, window, 1.0, point(px(0.), px(0.)));
        assert_eq!(centered.size.width, px(400.));
        assert_eq!(centered.size.height, px(300.));
        assert_eq!(centered.origin.x, px(300.));
        assert_eq!(centered.origin.y, px(200.));

        // Zoom twice, and the picture is twice as big, still centred.
        let zoomed = viewer_bounds(image, window, 2.0, point(px(0.), px(0.)));
        assert_eq!(zoomed.size.width, px(800.));
        assert_eq!(zoomed.origin.x, px(100.));

        // A drag moves it by exactly what the pointer travelled.
        let dragged = viewer_bounds(image, window, 1.0, point(px(40.), px(-25.)));
        assert_eq!(dragged.origin.x, centered.origin.x + px(40.));
        assert_eq!(dragged.origin.y, centered.origin.y - px(25.));
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer_still() {
        let center = point(px(500.), px(350.));
        // A point 100 right and 50 above the middle, with the picture centred.
        let anchor = point(px(600.), px(300.));
        let offset = zoomed_offset(point(px(0.), px(0.)), anchor, center, 2.0);
        // Doubling the picture would push that point out by its own distance
        // from the middle, so the picture moves the other way by as much.
        assert_eq!(offset.x, px(-100.));
        assert_eq!(offset.y, px(50.));

        // Which is exactly where the point stays: its position on screen is the
        // middle plus the offset plus the distance scaled by the zoom.
        let after_x = center.x + offset.x + (anchor.x - center.x) * 2.0;
        assert_eq!(after_x, anchor.x);
    }

    #[test]
    fn the_arrows_wrap_at_both_ends() {
        assert_eq!(wrapped_index(0, -1, 3), 2);
        assert_eq!(wrapped_index(2, 1, 3), 0);
        assert_eq!(wrapped_index(1, 1, 3), 2);
        assert_eq!(wrapped_index(1, -2, 3), 2);
        // One picture, or none: there is nowhere else to go.
        assert_eq!(wrapped_index(0, 1, 1), 0);
        assert_eq!(wrapped_index(0, -1, 0), 0);
    }

    #[test]
    fn the_language_list_is_the_one_the_fences_are_coloured_with() {
        let all = lang_matches("");
        // The set comes from syntect, not from a hand-kept list, so it is large
        // and holds the everyday names.
        assert!(all.len() > 80, "expected a full syntax set, got {}", all.len());
        for token in ["rs", "py", "js", "md", "toml", "css"] {
            assert!(
                all.iter().any(|choice| choice.token == token),
                "{token} should be offered"
            );
        }
        // The fallback is the absence of a language, not a language.
        assert!(all.iter().all(|choice| choice.name != "Plain Text"));
        // Nothing is offered twice, so no row is a duplicate of another.
        let mut tokens: Vec<&str> = all.iter().map(|choice| choice.token.as_str()).collect();
        tokens.sort_unstable();
        let before = tokens.len();
        tokens.dedup();
        assert_eq!(before, tokens.len());
    }

    #[test]
    fn a_typed_language_narrows_the_list() {
        let all = lang_matches("").len();
        let rusty = lang_matches("rus");
        assert!(rusty.len() < all);
        assert_eq!(rusty.first().map(|choice| choice.name.as_str()), Some("Rust"));

        // A fence token is matched as readily as a display name: "rs" is what a
        // reader types into the fence, "Rust" is what the list shows.
        let short = lang_matches("rs");
        assert_eq!(short.first().map(|choice| choice.token.as_str()), Some("rs"));
        assert!(lang_matches("zzzznotalanguage").is_empty());
    }
}

