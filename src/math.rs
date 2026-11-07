//! A small LaTeX math engine: source in, positioned glyphs out.
//!
//! It is deliberately not a TeX implementation. It covers what people actually
//! type in notes — greek letters, operators, superscripts, subscripts,
//! fractions, roots, big operators, matrices and common accents — and lays them
//! out with hand-rolled metrics so a formula can be drawn anywhere: centred on
//! its own line for `$$…$$`, or inline in a sentence for `$…$`.
//!
//! The renderer draws shaped lines rather than text elements. A fraction needs a
//! shared baseline across boxes of different sizes, which the element tree
//! cannot express, while `ShapedLine::paint` takes an exact origin and leaves
//! the vertical maths to us.

use gpui_kit::*;

/// One glyph run or rule, already positioned inside the box.
enum Item {
    Text {
        line: ShapedLine,
        x: f32,
        /// Distance from the top of the box to this run's baseline.
        baseline: f32,
        ascent: f32,
        descent: f32,
    },
    Rule {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
}

/// A laid-out formula, in pixels.
pub struct MathBox {
    pub width: f32,
    pub height: f32,
    /// Distance from the top to the baseline of the last row: inline formulas
    /// line up with the surrounding text on it.
    pub baseline: f32,
    pub color: Hsla,
    items: Vec<Item>,
}

impl MathBox {
    /// Draw the formula with its top-left corner at `origin`.
    pub fn paint(&self, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
        for item in &self.items {
            match item {
                Item::Text {
                    line,
                    x,
                    baseline,
                    ascent,
                    descent,
                } => {
                    let top = origin.y + px(*baseline) - px(*ascent);
                    // `line_height` = ascent + descent means no half-leading, so
                    // the baseline lands exactly at `top + ascent`.
                    let _ = line.paint(
                        point(origin.x + px(*x), top),
                        px(*ascent + *descent),
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
                Item::Rule {
                    x,
                    y,
                    width,
                    height,
                } => window.paint_quad(fill(
                    Bounds {
                        origin: origin + point(px(*x), px(*y)),
                        size: size(px(*width), px(*height)),
                    },
                    self.color,
                )),
            }
        }
    }

}

// --------------------------------------------------------------------- parsing

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    /// A single variable: italic, the way maths is set.
    Var,
    /// Digits, punctuation, upright.
    Num,
    /// Function names and `\text{}`: upright.
    Roman,
    Bold,
}

#[derive(Clone)]
enum Node {
    Row(Vec<Node>),
    Run(&'static str),
    Text(String, Style),
    Space(f32),
    Frac(Box<Node>, Box<Node>),
    Sqrt(Box<Node>),
    Script {
        base: Box<Node>,
        sup: Option<Box<Node>>,
        sub: Option<Box<Node>>,
    },
    Big {
        text: &'static str,
        sup: Option<Box<Node>>,
        sub: Option<Box<Node>>,
    },
    /// `\left( … \right)`, with the delimiters grown to the content.
    Delim {
        left: &'static str,
        body: Box<Node>,
        right: &'static str,
    },
    Matrix {
        rows: Vec<Vec<Node>>,
        left: &'static str,
        right: &'static str,
    },
}

/// What a command turns into.
enum Cmd {
    Symbol(&'static str),
    /// A symbol that behaves like an operator and gets space on both sides.
    Binary(&'static str),
    Relation(&'static str),
    Function(&'static str),
    Big(&'static str),
    Space(f32),
    Delimiter(&'static str),
    Text,
    Frac,
    Sqrt,
    Begin,
    End,
    Left,
    Right,
    Break,
    Escaped(char),
}

fn command(name: &str) -> Option<Cmd> {
    use Cmd::*;
    Some(match name {
        // ---- greek
        "alpha" => Symbol("α"),
        "beta" => Symbol("β"),
        "gamma" => Symbol("γ"),
        "delta" => Symbol("δ"),
        "epsilon" => Symbol("ε"),
        "varepsilon" => Symbol("ε"),
        "zeta" => Symbol("ζ"),
        "eta" => Symbol("η"),
        "theta" => Symbol("θ"),
        "vartheta" => Symbol("ϑ"),
        "iota" => Symbol("ι"),
        "kappa" => Symbol("κ"),
        "lambda" => Symbol("λ"),
        "mu" => Symbol("μ"),
        "nu" => Symbol("ν"),
        "xi" => Symbol("ξ"),
        "pi" => Symbol("π"),
        "rho" => Symbol("ρ"),
        "sigma" => Symbol("σ"),
        "tau" => Symbol("τ"),
        "upsilon" => Symbol("υ"),
        "phi" => Symbol("φ"),
        "varphi" => Symbol("ϕ"),
        "chi" => Symbol("χ"),
        "psi" => Symbol("ψ"),
        "omega" => Symbol("ω"),
        "Gamma" => Symbol("Γ"),
        "Delta" => Symbol("Δ"),
        "Theta" => Symbol("Θ"),
        "Lambda" => Symbol("Λ"),
        "Xi" => Symbol("Ξ"),
        "Pi" => Symbol("Π"),
        "Sigma" => Symbol("Σ"),
        "Upsilon" => Symbol("Υ"),
        "Phi" => Symbol("Φ"),
        "Psi" => Symbol("Ψ"),
        "Omega" => Symbol("Ω"),
        // ---- binary operators
        "pm" => Binary("±"),
        "mp" => Binary("∓"),
        "times" => Binary("×"),
        "div" => Binary("÷"),
        "cdot" => Binary("·"),
        "ast" => Binary("∗"),
        "star" => Binary("⋆"),
        "circ" => Binary("∘"),
        "bullet" => Binary("•"),
        "oplus" => Binary("⊕"),
        "otimes" => Binary("⊗"),
        "wedge" => Binary("∧"),
        "vee" => Binary("∨"),
        // ---- relations
        "le" | "leq" => Relation("≤"),
        "ge" | "geq" => Relation("≥"),
        "ne" | "neq" => Relation("≠"),
        "approx" => Relation("≈"),
        "sim" => Relation("∼"),
        "simeq" => Relation("≃"),
        "cong" => Relation("≅"),
        "equiv" => Relation("≡"),
        "propto" => Relation("∝"),
        "in" => Relation("∈"),
        "notin" => Relation("∉"),
        "subset" => Relation("⊂"),
        "supset" => Relation("⊃"),
        "subseteq" => Relation("⊆"),
        "supseteq" => Relation("⊇"),
        "cup" => Relation("∪"),
        "cap" => Relation("∩"),
        "to" | "rightarrow" => Relation("→"),
        "leftarrow" => Relation("←"),
        "Rightarrow" | "implies" => Relation("⇒"),
        "Leftarrow" => Relation("⇐"),
        "leftrightarrow" => Relation("↔"),
        "iff" | "Leftrightarrow" => Relation("⟺"),
        "mapsto" => Relation("↦"),
        "perp" => Relation("⊥"),
        "parallel" => Relation("∥"),
        // ---- ordinary symbols
        "infty" => Symbol("∞"),
        "partial" => Symbol("∂"),
        "nabla" => Symbol("∇"),
        "forall" => Symbol("∀"),
        "exists" => Symbol("∃"),
        "emptyset" => Symbol("∅"),
        "angle" => Symbol("∠"),
        "therefore" => Symbol("∴"),
        "because" => Symbol("∵"),
        "prime" => Symbol("′"),
        "degree" => Symbol("°"),
        "hbar" => Symbol("ℏ"),
        "ell" => Symbol("ℓ"),
        "dots" | "ldots" => Symbol("…"),
        "cdots" => Symbol("⋯"),
        "vdots" => Symbol("⋮"),
        "ddots" => Symbol("⋱"),
        "aleph" => Symbol("ℵ"),
        "Re" => Symbol("ℜ"),
        "Im" => Symbol("ℑ"),
        // ---- functions
        "sin" => Function("sin"),
        "cos" => Function("cos"),
        "tan" => Function("tan"),
        "cot" => Function("cot"),
        "sec" => Function("sec"),
        "csc" => Function("csc"),
        "arcsin" => Function("arcsin"),
        "arccos" => Function("arccos"),
        "arctan" => Function("arctan"),
        "sinh" => Function("sinh"),
        "cosh" => Function("cosh"),
        "tanh" => Function("tanh"),
        "log" => Function("log"),
        "ln" => Function("ln"),
        "lg" => Function("lg"),
        "exp" => Function("exp"),
        "det" => Function("det"),
        "dim" => Function("dim"),
        "deg" => Function("deg"),
        "gcd" => Function("gcd"),
        "mod" => Function("mod"),
        // ---- big operators
        "sum" => Big("∑"),
        "prod" => Big("∏"),
        "coprod" => Big("∐"),
        "int" => Big("∫"),
        "iint" => Big("∬"),
        "iiint" => Big("∭"),
        "oint" => Big("∮"),
        "bigcup" => Big("⋃"),
        "bigcap" => Big("⋂"),
        "bigoplus" => Big("⨁"),
        "lim" => Big("lim"),
        "max" => Big("max"),
        "min" => Big("min"),
        "sup" => Big("sup"),
        "inf" => Big("inf"),
        // ---- spacing
        "," => Space(0.167),
        ":" => Space(0.222),
        ";" => Space(0.278),
        "!" => Space(-0.167),
        "quad" => Space(1.0),
        "qquad" => Space(2.0),
        " " => Space(0.25),
        // ---- delimiters
        "langle" => Delimiter("⟨"),
        "rangle" => Delimiter("⟩"),
        "lfloor" => Delimiter("⌊"),
        "rfloor" => Delimiter("⌋"),
        "lceil" => Delimiter("⌈"),
        "rceil" => Delimiter("⌉"),
        "lbrace" => Delimiter("{"),
        "rbrace" => Delimiter("}"),
        "vert" | "mid" => Delimiter("|"),
        "Vert" => Delimiter("‖"),
        "left" => Left,
        "right" => Right,
        // ---- structure
        "frac" | "dfrac" | "tfrac" => Frac,
        "sqrt" => Sqrt,
        "text" | "textrm" | "mathrm" | "operatorname" | "mbox" => Text,
        "begin" => Begin,
        "end" => End,
        "\\" => Break,
        "{" => Escaped('{'),
        "}" => Escaped('}'),
        "%" => Escaped('%'),
        "$" => Escaped('$'),
        "&" => Escaped('&'),
        "#" => Escaped('#'),
        "_" => Escaped('_'),
        "|" => Delimiter("‖"),
        _ => return None,
    })
}

struct Parser {
    chars: Vec<char>,
    at: usize,
}

impl Parser {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            at: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.at += 1;
        Some(ch)
    }

    /// Read `{…}` and parse what is inside; a bare single token is accepted too.
    fn group(&mut self) -> Node {
        self.skip_space();
        if self.peek() == Some('{') {
            self.at += 1;
            let node = self.row(&['}']);
            if self.peek() == Some('}') {
                self.at += 1;
            }
            return node;
        }
        // `x^\infty` and `\sum_i` put a whole command where a group could go.
        if self.peek() == Some('\\') {
            return self.command_atom();
        }
        match self.atom() {
            Some(node) => node,
            None => Node::Row(Vec::new()),
        }
    }

    /// One command used as a value rather than as a prefix, e.g. `^\infty`.
    fn command_atom(&mut self) -> Node {
        let name = self.command_name();
        match command(&name) {
            Some(Cmd::Symbol(text)) | Some(Cmd::Binary(text)) | Some(Cmd::Relation(text)) => {
                Node::Run(text)
            }
            Some(Cmd::Escaped(ch)) => Node::Text(ch.to_string(), Style::Num),
            Some(Cmd::Delimiter(text)) => Node::Run(text),
            Some(Cmd::Function(name)) => Node::Text(name.to_string(), Style::Roman),
            Some(Cmd::Space(em)) => Node::Space(em),
            Some(Cmd::Text) => {
                let text = self.raw_group();
                Node::Text(text, Style::Roman)
            }
            Some(Cmd::Frac) => {
                let num = self.group();
                let den = self.group();
                Node::Frac(Box::new(num), Box::new(den))
            }
            Some(Cmd::Sqrt) => {
                let body = self.group();
                Node::Sqrt(Box::new(body))
            }
            Some(Cmd::Big(text)) => Node::Big {
                text,
                sup: None,
                sub: None,
            },
            _ => Node::Text(format!("\\{name}"), Style::Roman),
        }
    }

    /// Text inside `\text{}`: taken verbatim, no maths inside.
    fn raw_group(&mut self) -> String {
        self.skip_space();
        if self.peek() != Some('{') {
            return self.bump().map(String::from).unwrap_or_default();
        }
        self.at += 1;
        let mut out = String::new();
        let mut depth = 1;
        while let Some(ch) = self.bump() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            out.push(ch);
        }
        out
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t') | Some('\n') | Some('\r')) {
            self.at += 1;
        }
    }

    fn row(&mut self, stop: &[char]) -> Node {
        let mut nodes: Vec<Node> = Vec::new();
        loop {
            self.skip_space();
            let Some(ch) = self.peek() else {
                break;
            };
            if stop.contains(&ch) || ch == '&' {
                break;
            }
            if ch == '\\' {
                // `\\` ends a row, `\end` ends the environment.
                if self.chars.get(self.at + 1) == Some(&'\\') {
                    self.at += 2;
                    nodes.push(Node::Text("\\break".into(), Style::Num));
                    continue;
                }
                let name = self.command_name();
                if name == "end" {
                    self.at -= 4;
                    break;
                }
                let Some(cmd) = command(&name) else {
                    // Unknown command: show it as upright text so nothing is lost.
                    nodes.push(Node::Text(format!("\\{name}"), Style::Roman));
                    continue;
                };
                match cmd {
                    Cmd::Symbol(text) => nodes.push(Node::Run(text)),
                    Cmd::Binary(text) => {
                        nodes.push(Node::Space(0.22));
                        nodes.push(Node::Run(text));
                        nodes.push(Node::Space(0.22));
                    }
                    Cmd::Relation(text) => {
                        nodes.push(Node::Space(0.28));
                        nodes.push(Node::Run(text));
                        nodes.push(Node::Space(0.28));
                    }
                    Cmd::Function(name) => {
                        if matches!(nodes.last(), Some(Node::Text(_, _)) | Some(Node::Run(_))) {
                            nodes.push(Node::Space(0.16));
                        }
                        nodes.push(Node::Text(name.to_string(), Style::Roman));
                        nodes.push(Node::Space(0.1));
                    }
                    Cmd::Big(text) => {
                        let (sub, sup) = self.scripts();
                        nodes.push(Node::Big { text, sub, sup });
                    }
                    Cmd::Space(em) => nodes.push(Node::Space(em)),
                    Cmd::Delimiter(text) => nodes.push(Node::Run(text)),
                    Cmd::Escaped(ch) => nodes.push(Node::Text(ch.to_string(), Style::Num)),
                    Cmd::Text => {
                        let text = self.raw_group();
                        nodes.push(Node::Text(text, Style::Roman));
                    }
                    Cmd::Frac => {
                        let num = self.group();
                        let den = self.group();
                        nodes.push(Node::Frac(Box::new(num), Box::new(den)));
                    }
                    Cmd::Sqrt => {
                        // An optional index `[n]` is dropped: the root sign is
                        // what readers look for.
                        self.skip_space();
                        if self.peek() == Some('[') {
                            while let Some(ch) = self.bump() {
                                if ch == ']' {
                                    break;
                                }
                            }
                        }
                        let body = self.group();
                        nodes.push(Node::Sqrt(Box::new(body)));
                    }
                    Cmd::Left => {
                        self.skip_space();
                        let open = self.delimiter_char();
                        // Everything up to `\right` is the parenthesised part.
                        let body = self.row(&[]);
                        self.skip_space();
                        if self.peek() == Some('\\') {
                            let name = self.command_name();
                            if name != "right" {
                                nodes.push(body);
                                continue;
                            }
                        }
                        self.skip_space();
                        let close = self.delimiter_char();
                        nodes.push(Node::Delim {
                            left: open,
                            body: Box::new(body),
                            right: close,
                        });
                    }
                    Cmd::Right => {
                        // Stray `\right`, the body was already taken.
                    }
                    Cmd::Begin => {
                        let name = self.raw_group();
                        let node = self.environment(&name);
                        nodes.push(node);
                    }
                    Cmd::End => break,
                    Cmd::Break => nodes.push(Node::Text("\\break".into(), Style::Num)),
                }
                continue;
            }
            if ch == '^' || ch == '_' {
                let (sub, sup) = self.scripts();
                let base = nodes
                    .pop()
                    .unwrap_or_else(|| Node::Row(Vec::new()));
                nodes.push(Node::Script {
                    base: Box::new(base),
                    sup,
                    sub,
                });
                continue;
            }
            if ch == '{' {
                self.at += 1;
                let inner = self.row(&['}']);
                if self.peek() == Some('}') {
                    self.at += 1;
                }
                nodes.push(inner);
                continue;
            }
            if ch == '}' {
                self.at += 1;
                continue;
            }
            let Some(node) = self.atom() else {
                break;
            };
            nodes.push(node);
        }
        Node::Row(nodes)
    }

    /// Read `^{…}` and/or `_{…}` in either order.
    fn scripts(&mut self) -> (Option<Box<Node>>, Option<Box<Node>>) {
        let mut sub = None;
        let mut sup = None;
        loop {
            self.skip_space();
            match self.peek() {
                Some('^') => {
                    self.at += 1;
                    sup = Some(Box::new(self.group()));
                }
                Some('_') => {
                    self.at += 1;
                    sub = Some(Box::new(self.group()));
                }
                _ => break,
            }
        }
        (sub, sup)
    }

    fn command_name(&mut self) -> String {
        // Sitting on the backslash.
        self.at += 1;
        let mut name = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphabetic() {
                name.push(ch);
                self.at += 1;
            } else {
                break;
            }
        }
        if name.is_empty() {
            // `\,` `\{` and friends: the command *is* the next character.
            if let Some(ch) = self.bump() {
                name.push(ch);
            }
        }
        name
    }

    fn delimiter_char(&mut self) -> &'static str {
        match self.peek() {
            Some('.') => {
                self.at += 1;
                ""
            }
            Some('\\') => {
                let name = self.command_name();
                match command(&name) {
                    Some(Cmd::Delimiter(text)) => text,
                    Some(Cmd::Symbol(text)) => text,
                    _ => "|",
                }
            }
            Some('(') => {
                self.at += 1;
                "("
            }
            Some(')') => {
                self.at += 1;
                ")"
            }
            Some('[') => {
                self.at += 1;
                "["
            }
            Some(']') => {
                self.at += 1;
                "]"
            }
            Some('|') => {
                self.at += 1;
                "|"
            }
            Some('{') => {
                self.at += 1;
                "{"
            }
            Some('}') => {
                self.at += 1;
                "}"
            }
            Some('<') => {
                self.at += 1;
                "⟨"
            }
            Some('>') => {
                self.at += 1;
                "⟩"
            }
            _ => "",
        }
    }

    /// A single character of maths: a digit, a variable, or an operator.
    fn atom(&mut self) -> Option<Node> {
        let ch = self.peek()?;
        match ch {
            '0'..='9' | '.' => {
                let mut text = String::new();
                while let Some(ch) = self.peek() {
                    if ch.is_ascii_digit() || ch == '.' {
                        text.push(ch);
                        self.at += 1;
                    } else {
                        break;
                    }
                }
                Some(Node::Text(text, Style::Num))
            }
            '+' | '-' => {
                self.at += 1;
                Some(Node::Row(vec![
                    Node::Space(0.22),
                    Node::Run(if ch == '+' { "+" } else { "−" }),
                    Node::Space(0.22),
                ]))
            }
            '=' => {
                self.at += 1;
                Some(Node::Row(vec![
                    Node::Space(0.28),
                    Node::Run("="),
                    Node::Space(0.28),
                ]))
            }
            '<' => {
                self.at += 1;
                Some(Node::Row(vec![
                    Node::Space(0.28),
                    Node::Run("<"),
                    Node::Space(0.28),
                ]))
            }
            '>' => {
                self.at += 1;
                Some(Node::Row(vec![
                    Node::Space(0.28),
                    Node::Run(">"),
                    Node::Space(0.28),
                ]))
            }
            '\'' => {
                self.at += 1;
                Some(Node::Run("′"))
            }
            '&' | '}' => None,
            _ if ch.is_alphabetic() => {
                self.at += 1;
                Some(Node::Text(ch.to_string(), Style::Var))
            }
            _ => {
                self.at += 1;
                let text: String = ch.to_string();
                Some(Node::Text(text, Style::Num))
            }
        }
    }

    /// `\begin{env} … \end{env}`.
    fn environment(&mut self, name: &str) -> Node {
        let (left, right) = match name {
            "pmatrix" => ("(", ")"),
            "bmatrix" => ("[", "]"),
            "vmatrix" => ("|", "|"),
            "Bmatrix" => ("{", "}"),
            "cases" => ("{", ""),
            _ => ("", ""),
        };
        let mut rows: Vec<Vec<Node>> = Vec::new();
        let mut cells: Vec<Node> = Vec::new();
        loop {
            let cell = self.row(&['&', '\\']);
            cells.push(cell);
            self.skip_space();
            match self.peek() {
                Some('&') => {
                    self.at += 1;
                    continue;
                }
                Some('\\') if self.chars.get(self.at + 1) == Some(&'\\') => {
                    self.at += 2;
                    rows.push(std::mem::take(&mut cells));
                    continue;
                }
                Some('\\') => {
                    let name = self.command_name();
                    if name == "end" {
                        let _ = self.raw_group();
                        break;
                    }
                    continue;
                }
                _ => break,
            }
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
        Node::Matrix { rows, left, right }
    }
}

/// Parse a whole formula, splitting `\\` into separate rows.
fn parse(source: &str) -> Vec<Node> {
    let mut parser = Parser::new(source);
    let mut rows: Vec<Node> = Vec::new();
    loop {
        let row = parser.row(&[]);
        rows.push(row);
        parser.skip_space();
        match parser.peek() {
            Some('\\') if parser.chars.get(parser.at + 1) == Some(&'\\') => {
                parser.at += 2;
            }
            _ => break,
        }
    }
    // `\\` inside a plain formula shows up as a marker atom; cut the rows again
    // on it so a two-line aligned formula works without an environment.
    let mut split: Vec<Node> = Vec::new();
    for row in rows {
        split.extend(split_on_break(row));
    }
    split
}

fn split_on_break(node: Node) -> Vec<Node> {
    match node {
        Node::Row(items) => {
            let mut out: Vec<Vec<Node>> = vec![Vec::new()];
            for item in items {
                if matches!(&item, Node::Text(text, _) if text == "\\break") {
                    out.push(Vec::new());
                    continue;
                }
                if let Node::Row(inner) = item {
                    let pieces = split_on_break(Node::Row(inner));
                    if pieces.len() > 1 {
                        let mut pieces = pieces.into_iter();
                        if let Some(first) = pieces.next() {
                            out.last_mut().unwrap().push(first);
                        }
                        for piece in pieces {
                            out.push(vec![piece]);
                        }
                        continue;
                    }
                    out.last_mut().unwrap().push(pieces.into_iter().next().unwrap());
                    continue;
                }
                out.last_mut().unwrap().push(item);
            }
            out.into_iter().map(Node::Row).collect()
        }
        other => vec![other],
    }
}

// -------------------------------------------------------------------- laying out

/// A laid-out fragment, measured from its own baseline.
struct Piece {
    width: f32,
    ascent: f32,
    descent: f32,
    glyphs: Vec<Glyph>,
    rules: Vec<Rule>,
}

struct Glyph {
    text: String,
    x: f32,
    /// Baseline offset from the fragment's baseline (negative is up).
    y: f32,
    size: f32,
    style: Style,
}

struct Rule {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl Piece {
    fn empty() -> Self {
        Self {
            width: 0.,
            ascent: 0.,
            descent: 0.,
            glyphs: Vec::new(),
            rules: Vec::new(),
        }
    }

    fn shift(&mut self, dx: f32, dy: f32) {
        for glyph in &mut self.glyphs {
            glyph.x += dx;
            glyph.y += dy;
        }
        for rule in &mut self.rules {
            rule.x += dx;
            rule.y += dy;
        }
    }

    fn append(&mut self, mut other: Piece) {
        other.shift(self.width, 0.);
        self.width += other.width;
        self.ascent = self.ascent.max(other.ascent);
        self.descent = self.descent.max(other.descent);
        self.glyphs.extend(other.glyphs);
        self.rules.extend(other.rules);
    }
}

struct Engine<'a> {
    display: bool,
    family: &'a str,
    color: Hsla,
    window: &'a mut Window,
}

impl<'a> Engine<'a> {
    fn measure(&mut self, text: &str, size: f32, style: Style) -> (f32, f32, f32) {
        let run = self.run(text, size, style);
        let shaped = self.window.text_system().shape_text(
            SharedString::from(text.to_string()),
            px(size),
            std::slice::from_ref(&run),
            None,
            None,
        );
        match shaped {
            Ok(lines) => lines.first().map_or(
                (size * 0.5 * text.chars().count() as f32, size * 0.75, size * 0.25),
                |line| {
                    (
                        line.width().as_f32(),
                        line.unwrapped_layout.ascent.as_f32(),
                        line.unwrapped_layout.descent.as_f32(),
                    )
                },
            ),
            Err(_) => (
                size * 0.55 * text.chars().count() as f32,
                size * 0.75,
                size * 0.25,
            ),
        }
    }

    fn run(&self, text: &str, size: f32, style: Style) -> TextRun {
        let _ = size;
        TextRun {
            len: text.len(),
            font: Font {
                family: self.family.into(),
                weight: if style == Style::Bold {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                },
                style: if style == Style::Var {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                },
                ..Default::default()
            },
            color: self.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    fn text(&mut self, text: String, size: f32, style: Style) -> Piece {
        let (width, ascent, descent) = self.measure(&text, size, style);
        Piece {
            width,
            ascent,
            descent,
            glyphs: vec![Glyph {
                text,
                x: 0.,
                y: 0.,
                size,
                style,
            }],
            rules: Vec::new(),
        }
    }

    fn layout(&mut self, node: &Node, size: f32) -> Piece {
        let axis = size * 0.25;
        match node {
            Node::Row(items) => {
                let mut piece = Piece::empty();
                for item in items {
                    let child = self.layout(item, size);
                    piece.append(child);
                }
                piece
            }
            Node::Run(text) => self.text(text.to_string(), size, Style::Num),
            Node::Text(text, style) => {
                let text = text.clone();
                self.text(text, size, *style)
            }
            Node::Space(em) => Piece {
                width: size * em,
                ascent: 0.,
                descent: 0.,
                glyphs: Vec::new(),
                rules: Vec::new(),
            },
            Node::Frac(num, den) => {
                let inner = size * 0.85;
                let mut numerator = self.layout(num, inner);
                let mut denominator = self.layout(den, inner);
                let thickness = (size * 0.055).max(1.);
                let gap = size * 0.15;
                let padding = size * 0.1;
                let width = numerator.width.max(denominator.width) + padding * 2.;
                let dx_num = (width - numerator.width) / 2.;
                let dx_den = (width - denominator.width) / 2.;
                numerator.shift(dx_num, 0.);
                denominator.shift(dx_den, 0.);
                // The bar sits on the maths axis; each half clears it by `gap`.
                let bar_top = -axis - thickness;
                let num_baseline = bar_top - gap - numerator.descent;
                let den_baseline = bar_top + thickness + gap + denominator.ascent;
                let mut out = Piece {
                    width,
                    ascent: -num_baseline + numerator.ascent,
                    descent: den_baseline + denominator.descent,
                    glyphs: Vec::new(),
                    rules: vec![Rule {
                        x: 0.,
                        y: bar_top,
                        width,
                        height: thickness,
                    }],
                };
                let mut numer = numerator;
                for glyph in &mut numer.glyphs {
                    glyph.y += num_baseline;
                }
                for rule in &mut numer.rules {
                    rule.y += num_baseline;
                }
                out.glyphs.extend(numer.glyphs);
                out.rules.extend(numer.rules);
                let mut denom = denominator;
                for glyph in &mut denom.glyphs {
                    glyph.y += den_baseline;
                }
                for rule in &mut denom.rules {
                    rule.y += den_baseline;
                }
                out.glyphs.extend(denom.glyphs);
                out.rules.extend(denom.rules);
                out
            }
            Node::Sqrt(body) => {
                let inner = self.layout(body, size);
                let pad = size * 0.2;
                let thickness = (size * 0.05).max(1.);
                let radical_size = (inner.ascent + inner.descent) * 1.05;
                let (radical_width, _radical_ascent, _radical_descent) =
                    self.measure("√", radical_size.max(inner.ascent + inner.descent), Style::Num);
                let mut out = Piece {
                    width: radical_width + inner.width + pad,
                    ascent: inner.ascent + pad * 0.6,
                    descent: inner.descent + pad * 0.2,
                    glyphs: Vec::new(),
                    rules: vec![Rule {
                        x: radical_width * 0.75,
                        y: -(inner.ascent + pad * 0.6) - thickness,
                        width: radical_width * 0.25 + inner.width + pad,
                        height: thickness,
                    }],
                };
                out.glyphs.push(Glyph {
                    text: "√".into(),
                    x: 0.,
                    y: 0.,
                    size: radical_size.max(inner.ascent + inner.descent),
                    style: Style::Num,
                });
                let mut body = inner;
                body.shift(radical_width + pad * 0.5, 0.);
                out.glyphs.extend(body.glyphs);
                out.rules.extend(body.rules);
                out
            }
            Node::Script { base, sup, sub } => {
                let mut base_piece = self.layout(base, size);
                let script_size = if self.display {
                    size * 0.68
                } else {
                    size * 0.62
                };
                let rise = size * 0.42;
                let drop = size * 0.24;
                let mut out = Piece {
                    width: base_piece.width,
                    ascent: base_piece.ascent,
                    descent: base_piece.descent,
                    glyphs: std::mem::take(&mut base_piece.glyphs),
                    rules: std::mem::take(&mut base_piece.rules),
                };
                if let Some(sup) = sup {
                    let mut piece = self.layout(sup, script_size);
                    let x = out.width - size * 0.04;
                    piece.shift(x, -rise);
                    out.width = out.width.max(x + piece.width);
                    out.ascent = out.ascent.max(rise + piece.ascent);
                    out.glyphs.extend(piece.glyphs);
                    out.rules.extend(piece.rules);
                }
                if let Some(sub) = sub {
                    let mut piece = self.layout(sub, script_size);
                    let x = out.width - size * 0.04;
                    piece.shift(x, drop);
                    out.width = out.width.max(x + piece.width);
                    out.descent = out.descent.max(drop + piece.ascent + piece.descent);
                    out.glyphs.extend(piece.glyphs);
                    out.rules.extend(piece.rules);
                }
                out
            }
            Node::Big { text, sup, sub } => {
                // A symbol (∑ ∫) is drawn large; a name (lim, max) is set in
                // roman at text size with its limit underneath, the way a book
                // sets it.
                let symbol = !text.chars().all(|ch| ch.is_ascii_alphabetic());
                let size_of_op = if symbol { size * 1.4 } else { size * 1.02 };
                let style = if symbol { Style::Num } else { Style::Roman };
                let mut piece = self.text(text.to_string(), size_of_op, style);
                let script_size = if self.display {
                    size * 0.66
                } else {
                    size * 0.6
                };
                // Integrals keep their limits beside them even on their own
                // line; everything else stacks in display mode.
                let stacked = self.display && !text.contains('∫');

                let mut limits: Vec<Piece> = Vec::new();
                if let Some(sup) = sup {
                    limits.push(self.layout(sup, script_size));
                }
                if let Some(sub) = sub {
                    limits.push(self.layout(sub, script_size));
                }

                if stacked {
                    let mut index = 0;
                    if sup.is_some() {
                        let mut limit = std::mem::replace(&mut limits[index], Piece::empty());
                        index += 1;
                        let width = piece.width.max(limit.width);
                        let y = -(piece.ascent + script_size * 0.62);
                        limit.shift((width - limit.width) / 2., y);
                        piece.ascent = piece.ascent.max(-y + limit.ascent);
                        piece.width = width;
                        piece.glyphs.extend(limit.glyphs);
                        piece.rules.extend(limit.rules);
                    }
                    if sub.is_some() {
                        let mut limit = std::mem::replace(&mut limits[index], Piece::empty());
                        let width = piece.width.max(limit.width);
                        let y = piece.descent + script_size * 1.2;
                        limit.shift((width - limit.width) / 2., y);
                        piece.descent = piece.descent.max(y + limit.ascent + limit.descent);
                        piece.width = width;
                        piece.glyphs.extend(limit.glyphs);
                        piece.rules.extend(limit.rules);
                    }
                } else {
                    let width = limits.iter().fold(0f32, |acc, l| acc.max(l.width));
                    let x = piece.width + size * 0.12;
                    let mut index = 0;
                    if sup.is_some() {
                        let mut limit = std::mem::replace(&mut limits[index], Piece::empty());
                        index += 1;
                        limit.shift(x, -size * 0.42);
                        piece.ascent = piece.ascent.max(size * 0.46 + limit.ascent);
                        piece.glyphs.extend(limit.glyphs);
                        piece.rules.extend(limit.rules);
                    }
                    if sub.is_some() {
                        let mut limit = std::mem::replace(&mut limits[index], Piece::empty());
                        limit.shift(x, size * 0.28);
                        piece.descent = piece.descent.max(size * 0.28 + limit.ascent);
                        piece.glyphs.extend(limit.glyphs);
                        piece.rules.extend(limit.rules);
                    }
                    piece.width = x + width;
                }
                piece.width += size * 0.28;
                piece
            }
            Node::Delim {
                left,
                body,
                right,
            } => {
                let mut inner = self.layout(body, size);
                let height = inner.ascent + inner.descent;
                let scale = (height / size).clamp(1., 2.4);
                let mut out = Piece {
                    width: 0.,
                    ascent: inner.ascent,
                    descent: inner.descent,
                    glyphs: Vec::new(),
                    rules: std::mem::take(&mut inner.rules),
                };
                if !left.is_empty() {
                    let mut piece = self.text(left.to_string(), size * scale, Style::Num);
                    piece.shift(0., 0.);
                    out.width += piece.width;
                    out.ascent = out.ascent.max(piece.ascent);
                    out.descent = out.descent.max(piece.descent);
                    out.glyphs.extend(piece.glyphs);
                }
                let mut body = inner;
                body.shift(out.width, 0.);
                out.width += body.width;
                out.glyphs.extend(body.glyphs);
                if !right.is_empty() {
                    let piece = self.text(right.to_string(), size * scale, Style::Num);
                    let mut piece = piece;
                    piece.shift(out.width, 0.);
                    out.width += piece.width;
                    out.glyphs.extend(piece.glyphs);
                }
                out
            }
            Node::Matrix { rows, left, right } => {
                let mut cells: Vec<Vec<Piece>> = Vec::new();
                let mut columns = 0;
                for row in rows {
                    let mut pieces: Vec<Piece> = Vec::new();
                    for cell in row {
                        pieces.push(self.layout(cell, size));
                    }
                    columns = columns.max(pieces.len());
                    cells.push(pieces);
                }
                let gap_x = size * 0.62;
                let gap_y = size * 0.34;
                let mut widths = vec![0f32; columns];
                for row in &cells {
                    for (index, piece) in row.iter().enumerate() {
                        widths[index] = widths[index].max(piece.width);
                    }
                }
                let total: f32 = widths.iter().sum::<f32>() + gap_x * (columns as f32 - 1.);
                let mut out = Piece::empty();
                let mut y = 0.;
                let mut first = true;
                for row in cells {
                    let row_ascent = row.iter().fold(0f32, |acc, piece| acc.max(piece.ascent));
                    let row_descent = row.iter().fold(0f32, |acc, piece| acc.max(piece.descent));
                    if !first {
                        y += row_ascent + gap_y;
                    }
                    let mut x = 0.;
                    for (index, piece) in row.into_iter().enumerate() {
                        let mut piece = piece;
                        let dx = x + (widths[index] - piece.width) / 2.;
                        piece.shift(dx, if first { 0. } else { y });
                        out.glyphs.extend(piece.glyphs);
                        out.rules.extend(piece.rules);
                        x += widths[index] + gap_x;
                    }
                    if first {
                        out.ascent = row_ascent;
                        out.descent = row_descent;
                        first = false;
                    } else {
                        out.descent = y + row_descent;
                    }
                }
                out.width = total;
                let scale = ((out.ascent + out.descent) / size).clamp(1., 2.4);
                let mut wrapped = Piece {
                    width: 0.,
                    ascent: out.ascent,
                    descent: out.descent,
                    glyphs: Vec::new(),
                    rules: std::mem::take(&mut out.rules),
                };
                if !left.is_empty() {
                    let mut piece = self.text(left.to_string(), size * scale, Style::Num);
                    piece.shift(0., 0.);
                    wrapped.width += piece.width;
                    wrapped.ascent = wrapped.ascent.max(piece.ascent);
                    wrapped.descent = wrapped.descent.max(piece.descent);
                    wrapped.glyphs.extend(piece.glyphs);
                }
                let offset = wrapped.width;
                let mut body = out;
                body.shift(offset, 0.);
                wrapped.width += body.width;
                wrapped.glyphs.extend(body.glyphs);
                if !right.is_empty() {
                    let mut piece = self.text(right.to_string(), size * scale, Style::Num);
                    piece.shift(wrapped.width, 0.);
                    wrapped.width += piece.width;
                    wrapped.glyphs.extend(piece.glyphs);
                }
                wrapped
            }
        }
    }
}

/// Lay a formula out at `size`. `display` is true for `$$…$$`, where fractions
/// grow and big-operator limits go above and below instead of beside.
pub fn layout(
    source: &str,
    size: f32,
    display: bool,
    family: &str,
    color: Hsla,
    window: &mut Window,
) -> MathBox {
    let nodes = parse(source);
    let mut engine = Engine {
        display,
        family,
        color,
        window,
    };
    let mut rows: Vec<Piece> = Vec::new();
    for node in &nodes {
        rows.push(engine.layout(node, size));
    }
    let gap = size * 0.45;
    let width = rows.iter().fold(0f32, |acc, row| acc.max(row.width));
    let mut height = 0f32;
    let mut baseline = 0f32;
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            height += gap;
        }
        if index == rows.len() - 1 {
            baseline = height + row.ascent;
        }
        height += row.ascent + row.descent;
    }
    if height <= 0. {
        height = size;
    }

    // Turn baselines into box coordinates and shape each run for painting.
    let mut items: Vec<Item> = Vec::new();
    let mut y = 0f32;
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            y += gap;
        }
        let row_baseline = y + row.ascent;
        for glyph in &row.glyphs {
            let run = engine.run(&glyph.text, glyph.size, glyph.style);
            let Ok(lines) = engine.window.text_system().shape_text(
                SharedString::from(glyph.text.clone()),
                px(glyph.size),
                std::slice::from_ref(&run),
                None,
                None,
            ) else {
                continue;
            };
            let Some(line) = lines.first() else {
                continue;
            };
            let ascent = line.unwrapped_layout.ascent.as_f32();
            let descent = line.unwrapped_layout.descent.as_f32();
            let shaped = engine
                .window
                .text_system()
                .shape_line(SharedString::from(glyph.text.clone()), px(glyph.size), std::slice::from_ref(&run), None);
            items.push(Item::Text {
                line: shaped,
                x: glyph.x,
                baseline: row_baseline + glyph.y,
                ascent,
                descent,
            });
        }
        for rule in &row.rules {
            items.push(Item::Rule {
                x: rule.x,
                y: row_baseline + rule.y,
                width: rule.width,
                height: rule.height,
            });
        }
        y += row.ascent + row.descent;
    }

    MathBox {
        width,
        height,
        baseline,
        color,
        items,
    }
}

/// Byte ranges of inline `$…$` maths inside a line of Markdown, with the source
/// between the dollars (delimiters excluded).
///
/// `literal` holds the ranges that are inline code: a dollar in there is a
/// character the author wanted to show, not a delimiter, so it must not be
/// paired with a real formula further along the line.
pub fn inline_spans(
    text: &str,
    literal: &[std::ops::Range<usize>],
) -> Vec<(std::ops::Range<usize>, String)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        if bytes[at] != b'$' {
            at += 1;
            continue;
        }
        if let Some(code) = literal.iter().find(|code| code.start <= at && at < code.end) {
            at = code.end.max(at + 1);
            continue;
        }
        // `$$` is block maths, handled by the block parser.
        if bytes.get(at + 1) == Some(&b'$') {
            at += 2;
            continue;
        }
        // Find the closing dollar on the same line, ignoring escaped ones and
        // ones that sit inside inline code.
        let mut end = at + 1;
        let mut found = None;
        while end < bytes.len() {
            if let Some(code) = literal.iter().find(|code| code.start <= end && end < code.end) {
                end = code.end;
                continue;
            }
            if bytes[end] == b'\\' {
                end += 2;
                continue;
            }
            if bytes[end] == b'$' {
                if !text[at + 1..end].trim().is_empty() {
                    found = Some(end);
                }
                break;
            }
            end += 1;
        }
        let Some(end) = found else {
            at += 1;
            continue;
        };
        spans.push((at..end + 1, text[at + 1..end].to_string()));
        at = end + 1;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::inline_spans;

    #[test]
    fn finds_inline_math_and_skips_display_math() {
        let spans = inline_spans("质能方程 $E=mc^2$ 与 $a+b$", &[]);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].1, "E=mc^2");
        assert_eq!(spans[1].1, "a+b");
        assert!(inline_spans("$$x$$", &[]).is_empty());
        assert!(inline_spans("没有公式", &[]).is_empty());
        // An unclosed dollar is not maths.
        assert!(inline_spans("价格 $5 元", &[]).is_empty());
    }

    #[test]
    fn a_dollar_in_inline_code_is_not_a_delimiter() {
        // "用 `$` 包起来：$E=mc^2$" — the dollar inside the code span is the
        // one character the author wants to show.
        let text = "用 `$` 包起来：$E=mc^2$";
        let open = text.find('`').unwrap();
        let close = text.rfind('`').unwrap() + 1;
        let spans = inline_spans(text, &[open..close]);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].1, "E=mc^2");
    }
}

