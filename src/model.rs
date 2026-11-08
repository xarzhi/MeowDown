//! The document model: Markdown text <-> editable blocks.
//!
//! MeowDown edits a document as a list of typed blocks instead of one big text
//! buffer. Parsing splits Markdown into those blocks, serializing joins them
//! back into a normal `.md` file, so the on-disk format stays plain Markdown.

/// The type of a single block, which decides how it is rendered and edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    Bullet,
    Ordered,
    Todo(bool),
    Quote,
    /// A fenced code block, with an optional info string.
    Code(Option<String>),
    /// A `$$ … $$` display formula. The text holds the LaTeX source between the
    /// delimiters, so it stays editable as plain text.
    Math,
    /// A pipe table. The text holds the whole table as Markdown source, so the
    /// rows stay editable (and the file stays plain Markdown).
    Table,
    /// A standalone image line. The text holds the raw `![alt](src)` markdown,
    /// so the path stays editable as plain text.
    Image,
    /// A horizontal rule. Not editable, it only holds a position.
    Rule,
}

impl BlockKind {
    pub fn is_rule(&self) -> bool {
        matches!(self, Self::Rule)
    }

    pub fn is_code(&self) -> bool {
        matches!(self, Self::Code(_))
    }

    pub fn is_image(&self) -> bool {
        matches!(self, Self::Image)
    }

    pub fn is_math(&self) -> bool {
        matches!(self, Self::Math)
    }

    pub fn is_table(&self) -> bool {
        matches!(self, Self::Table)
    }

    /// Whether the block carries a marker (`-`, `>`, `#` …) shown in the gutter.
    pub fn is_marked(&self) -> bool {
        !matches!(self, Self::Paragraph | Self::Rule)
    }

    /// List blocks that continue an existing run when pressing Enter.
    pub fn continue_on_enter(&self) -> Option<BlockKind> {
        match self {
            Self::Bullet => Some(Self::Bullet),
            Self::Ordered => Some(Self::Ordered),
            Self::Todo(checked) => Some(Self::Todo(*checked)),
            Self::Quote => Some(Self::Quote),
            _ => None,
        }
    }

    fn same_list_family(&self, other: &BlockKind) -> bool {
        matches!(
            (self, other),
            (
                Self::Bullet | Self::Ordered | Self::Todo(_),
                Self::Bullet | Self::Ordered | Self::Todo(_)
            )
        )
    }

    /// The marker drawn in the gutter while the block is being edited.
    #[allow(dead_code)]
    pub fn badge(&self) -> String {
        match self {
            Self::Paragraph => String::new(),
            Self::Heading(level) => format!("H{level}"),
            Self::Bullet => "•".to_string(),
            Self::Ordered => "1.".to_string(),
            Self::Todo(false) => "☐".to_string(),
            Self::Todo(true) => "☑".to_string(),
            Self::Quote => "❝".to_string(),
            Self::Code(_) => "</>".to_string(),
            Self::Math => "∑".to_string(),
            Self::Table => "▦".to_string(),
            Self::Image => "IMG".to_string(),
            Self::Rule => String::new(),
        }
    }

    /// The Markdown source for this block and its text.
    pub fn to_markdown(&self, text: &str, ordinal: usize) -> String {
        match self {
            Self::Paragraph => text.to_string(),
            Self::Heading(level) => {
                let hashes = "#".repeat((*level).clamp(1, 6) as usize);
                if text.is_empty() {
                    hashes
                } else {
                    format!("{hashes} {text}")
                }
            }
            Self::Bullet => format!("- {text}"),
            Self::Ordered => format!("{ordinal}. {text}"),
            Self::Todo(checked) => {
                format!("- [{}] {text}", if *checked { 'x' } else { ' ' })
            }
            Self::Quote => format!("> {text}"),
            Self::Code(None) => format!("```\n{text}\n```"),
            Self::Code(Some(lang)) => format!("```{lang}\n{text}\n```"),
            Self::Math => format!("$$\n{text}\n$$"),
            Self::Table => text.to_string(),
            Self::Image => text.to_string(),
            Self::Rule => "---".to_string(),
        }
    }

}

/// A block without any UI state attached.
#[derive(Clone, Debug)]
pub struct PlainBlock {
    pub kind: BlockKind,
    pub text: String,
}

impl PlainBlock {
    pub fn new(kind: BlockKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

/// Split a Markdown document into blocks.
///
/// Blank lines are separators and produce no blocks: an empty paragraph only
/// exists while the user is editing one.
pub fn parse(source: &str) -> Vec<PlainBlock> {
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut blocks = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index].trim_end();
        let trimmed = line.trim_start();

        if trimmed.is_empty() {
            index += 1;
            continue;
        }

        if trimmed.starts_with("$$") {
            if let Some(body) = math_block(&lines, &mut index) {
                blocks.push(PlainBlock::new(BlockKind::Math, body));
                continue;
            }
        }

        // A pipe table: a header row, a row of dashes that says how each column
        // is aligned, then any number of body rows.
        if starts_table(&lines, index) {
            let mut body: Vec<&str> = Vec::new();
            while index < lines.len() {
                let candidate = lines[index].trim_end();
                if candidate.trim().is_empty() || !candidate.contains('|') {
                    break;
                }
                body.push(candidate);
                index += 1;
            }
            if body.len() >= 2 {
                blocks.push(PlainBlock::new(BlockKind::Table, body.join("\n")));
                continue;
            }
            index -= body.len();
        }

        if let Some(lang) = fence_language(trimmed) {
            index += 1;
            let mut body: Vec<&str> = Vec::new();
            while index < lines.len() {
                let candidate = lines[index].trim_start();
                if is_fence(candidate) {
                    index += 1;
                    break;
                }
                body.push(lines[index]);
                index += 1;
            }
            blocks.push(PlainBlock::new(BlockKind::Code(lang), body.join("\n")));
            continue;
        }

        // A line that is nothing but an image becomes its own block, so it can
        // be drawn as a picture instead of literal `![alt](src)` text.
        if parse_image(trimmed).is_some() {
            blocks.push(PlainBlock::new(BlockKind::Image, trimmed.to_string()));
            index += 1;
            continue;
        }

        let block = parse_line(trimmed);
        // Consecutive plain lines are one Markdown paragraph (soft line breaks),
        // so keep them in a single editable block.
        if block.kind == BlockKind::Paragraph {
            let mut text = block.text;
            while index + 1 < lines.len() {
                let next = lines[index + 1].trim_start();
                if next.is_empty()
                    || is_fence(next)
                    || is_rule(next)
                    || fence_language(next).is_some()
                    || next.starts_with("$$")
                    || starts_table(&lines, index + 1)
                {
                    break;
                }                let candidate = parse_line(next);
                if candidate.kind != BlockKind::Paragraph {
                    break;
                }
                text.push('\n');
                text.push_str(&candidate.text);
                index += 1;
            }
            blocks.push(PlainBlock::new(BlockKind::Paragraph, text));
            index += 1;
            continue;
        }

        blocks.push(block);
        index += 1;
    }

    blocks
}

/// A parsed `![alt](src "title")` reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub alt: String,
    pub src: String,
    pub title: Option<String>,
}

impl ImageRef {
    /// The canonical Markdown for this reference.
    pub fn to_markdown(&self) -> String {
        match &self.title {
            Some(title) => format!("![{}]({} \"{}\")", self.alt, self.src, title),
            None => format!("![{}]({})", self.alt, self.src),
        }
    }
}

/// Parse a string that consists of exactly one Markdown image.
pub fn parse_image(text: &str) -> Option<ImageRef> {
    let trimmed = text.trim();
    let rest = trimmed.strip_prefix("![")?;
    let close = rest.find("](")?;
    let alt = rest[..close].to_string();
    let after = rest[close + 2..].strip_suffix(')')?;
    let after = after.trim();
    if after.is_empty() {
        return None;
    }

    let (src, title) = if let Some(inner) = after.strip_prefix('<') {
        let end = inner.find('>')?;
        let src = inner[..end].to_string();
        let tail = inner[end + 1..].trim();
        (src, parse_title(tail))
    } else if let Some(index) = find_title_separator(after) {
        let src = after[..index].trim().to_string();
        let tail = after[index..].trim();
        (src, parse_title(tail))
    } else {
        (after.to_string(), None)
    };

    if src.trim().is_empty() {
        return None;
    }
    Some(ImageRef { alt, src, title })
}

/// Byte index of the space that separates the destination from its title.
fn find_title_separator(after: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (index, ch) in after.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ' ' | '\t' if depth == 0 => return Some(index),
            _ => {}
        }
    }
    None
}

fn parse_title(tail: &str) -> Option<String> {
    let tail = tail.trim();
    if tail.is_empty() {
        return None;
    }
    let quoted = tail
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| tail.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\'')));
    Some(quoted.unwrap_or(tail).to_string())
}

fn is_fence(line: &str) -> bool {
    line.trim_end().starts_with("```") && line.trim_end().trim_start_matches('`').is_empty()
}

/// Read a `$$ … $$` formula starting at `index`, advancing past it.
///
/// Both forms are accepted: `$$x^2$$` on one line, and a fenced block whose
/// closing `$$` sits on its own line or ends the last line of maths.
fn math_block(lines: &[&str], index: &mut usize) -> Option<String> {
    let after = lines[*index].trim_start().strip_prefix("$$")?;
    // One-line form.
    if let Some(body) = after.strip_suffix("$$") {
        *index += 1;
        return Some(body.trim().to_string());
    }
    if !after.trim().is_empty() {
        // `$$x` with no closing delimiter is just text.
        return None;
    }
    *index += 1;
    let mut body: Vec<String> = Vec::new();
    while *index < lines.len() {
        let candidate = lines[*index].trim_end();
        if candidate.trim() == "$$" {
            *index += 1;
            return Some(body.join("\n"));
        }
        if let Some(stripped) = candidate.strip_suffix("$$") {
            if !stripped.trim().is_empty() {
                body.push(stripped.trim_end().to_string());
            }
            *index += 1;
            return Some(body.join("\n"));
        }
        body.push(candidate.to_string());
        *index += 1;
    }
    // Unterminated: the block runs to the end of the document.
    Some(body.join("\n"))
}

fn fence_language(line: &str) -> Option<Option<String>> {
    let rest = line.strip_prefix("```")?;
    let lang = rest.trim();
    Some(if lang.is_empty() {
        None
    } else {
        Some(lang.to_string())
    })
}

/// A pipe table, split into the cells the editor draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableData {
    pub header: Vec<String>,
    pub align: Vec<ColumnAlign>,
    pub rows: Vec<Vec<String>>,
}

/// How one column lines its text up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnAlign {
    Left,
    Center,
    Right,
}

impl TableData {
    pub fn columns(&self) -> usize {
        self.header.len()
    }

    /// Write the table back as Markdown. Editing a cell changes the model, and
    /// the document is still a Markdown file, so the grid has to be able to
    /// turn itself back into the pipe table it came from.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(&row_line(&self.header));
        out.push('\n');
        let separator: Vec<String> = (0..self.columns())
            .map(|index| {
                match self.align.get(index).copied().unwrap_or(ColumnAlign::Left) {
                    ColumnAlign::Left => "---",
                    ColumnAlign::Center => ":---:",
                    ColumnAlign::Right => "---:",
                }
                .to_string()
            })
            .collect();
        out.push_str(&row_line(&separator));
        for row in &self.rows {
            out.push('\n');
            out.push_str(&row_line(row));
        }
        out
    }
}

/// One `| a | b |` line, with any bar inside a cell left escaped.
fn row_line(cells: &[String]) -> String {
    let joined: Vec<String> = cells
        .iter()
        .map(|cell| cell.replace('|', "\\|"))
        .collect();
    format!("| {} |", joined.join(" | "))
}

/// Is line `index` the head of a table? That needs a header row with a pipe in
/// it and a dash row right below that also carries pipes — otherwise it is just
/// a paragraph that happens to contain a bar, or a horizontal rule.
pub fn starts_table(lines: &[&str], index: usize) -> bool {
    let Some(header) = lines.get(index) else {
        return false;
    };
    if !header.contains('|') {
        return false;
    }
    let Some(separator) = lines.get(index + 1) else {
        return false;
    };
    if !separator.contains('|') {
        return false;
    }
    let cells = split_row(separator);
    !cells.is_empty() && cells.iter().all(|cell| is_dashes(cell))
}

fn is_dashes(cell: &str) -> bool {
    let cell = cell.trim();
    let core = cell.trim_start_matches(':').trim_end_matches(':');
    !core.is_empty() && core.chars().all(|ch| ch == '-')
}

/// Split one table line into cells, honouring `\|` as a literal bar.
fn split_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let body = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let body = body.strip_suffix('|').unwrap_or(body);
    let mut cells: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if chars.peek() == Some(&'|') => {
                chars.next();
                current.push('|');
            }
            '|' => {
                cells.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    cells.push(current.trim().to_string());
    cells
}

/// Read a table out of the Markdown source a `Table` block holds.
pub fn parse_table(source: &str) -> Option<TableData> {
    let lines: Vec<&str> = source.lines().collect();
    if lines.len() < 2 || !starts_table(&lines, 0) {
        return None;
    }
    let mut header = split_row(lines[0]);
    let separator = split_row(lines[1]);
    let columns = header.len().max(separator.len()).max(1);
    header.resize(columns, String::new());
    let align: Vec<ColumnAlign> = (0..columns)
        .map(|index| {
            let cell = separator.get(index).map(String::as_str).unwrap_or("");
            let cell = cell.trim();
            match (cell.starts_with(':'), cell.ends_with(':')) {
                (true, true) => ColumnAlign::Center,
                (false, true) => ColumnAlign::Right,
                _ => ColumnAlign::Left,
            }
        })
        .collect();
    let mut rows: Vec<Vec<String>> = Vec::new();
    for line in &lines[2..] {
        if line.trim().is_empty() {
            continue;
        }
        let mut cells = split_row(line);
        cells.resize(columns, String::new());
        rows.push(cells);
    }
    Some(TableData {
        header,
        align,
        rows,
    })
}

/// The Markdown for a fresh table: a header the user can type over and one
/// empty row, which is the shape a table has when you start one by hand.
pub fn table_template() -> String {
    "| 列 1 | 列 2 | 列 3 |\n| --- | --- | --- |\n|  |  |  |".to_string()
}

fn parse_line(line: &str) -> PlainBlock {
    if is_rule(line) {
        return PlainBlock::new(BlockKind::Rule, "");
    }
    if let Some((level, text)) = heading(line) {
        return PlainBlock::new(BlockKind::Heading(level), text);
    }
    if let Some((checked, text)) = todo(line) {
        return PlainBlock::new(BlockKind::Todo(checked), text);
    }
    if let Some(text) = bullet(line) {
        return PlainBlock::new(BlockKind::Bullet, text);
    }
    if let Some(text) = ordered(line) {
        return PlainBlock::new(BlockKind::Ordered, text);
    }
    if let Some(text) = quote(line) {
        return PlainBlock::new(BlockKind::Quote, text);
    }
    PlainBlock::new(BlockKind::Paragraph, line.trim_end())
}

fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() < 3 {
        return false;
    }
    let Some(marker) = trimmed.chars().next() else {
        return false;
    };
    matches!(marker, '-' | '*' | '_') && trimmed.chars().all(|c| c == marker)
}

fn heading(line: &str) -> Option<(u8, String)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    if rest.is_empty() {
        return Some((hashes as u8, String::new()));
    }
    let text = rest.strip_prefix(' ')?;
    Some((hashes as u8, text.trim_end().to_string()))
}

fn todo(line: &str) -> Option<(bool, String)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            let rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix("[ ] ") {
                return Some((false, after.to_string()));
            }
            if rest == "[ ]" {
                return Some((false, String::new()));
            }
            for done in ["[x] ", "[X] "] {
                if let Some(after) = rest.strip_prefix(done) {
                    return Some((true, after.to_string()));
                }
            }
            if rest == "[x]" || rest == "[X]" {
                return Some((true, String::new()));
            }
        }
    }
    None
}

fn bullet(line: &str) -> Option<String> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest.trim_end().to_string());
        }
    }
    None
}

fn ordered(line: &str) -> Option<String> {
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let rest = &line[digits..];
    let rest = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'))?;
    let rest = rest.strip_prefix(' ')?;
    Some(rest.trim_end().to_string())
}

fn quote(line: &str) -> Option<String> {
    let rest = line.strip_prefix('>')?;
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    Some(rest.trim_end().to_string())
}

/// Join blocks back into a Markdown document.
pub fn serialize(blocks: &[PlainBlock]) -> String {
    let mut out = String::new();
    let mut previous: Option<&BlockKind> = None;
    let mut ordinal = 0usize;

    for block in blocks {
        if block.kind == BlockKind::Ordered {
            ordinal += 1;
        } else {
            ordinal = 0;
        }

        if !out.is_empty() {
            let continues_list = previous.is_some_and(|prev| {
                prev.same_list_family(&block.kind) && block.kind.is_marked()
            });
            if !continues_list {
                out.push('\n');
            }
        }

        let markdown = block.kind.to_markdown(&block.text, ordinal.max(1));
        if markdown.trim().is_empty() {
            continue;
        }
        out.push_str(&markdown);
        out.push('\n');
        previous = Some(&block.kind);
    }

    out.trim_end_matches('\n').to_string() + "\n"
}

/// Strip inline formatting markers, keeping only the visible text (Ctrl+\\).
///
/// Nothing is stripped unless it forms a real pair, so plain text such as
/// `snake_case_name` or a lone `*` survives untouched.
pub fn strip_inline_markup(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut skip = vec![false; chars.len()];

    mark_links(&chars, &mut skip);
    mark_html_tags(&chars, &mut skip);

    for marker in ["***", "___", "**", "__", "~~", "==", "*", "_", "`"] {
        let seq: Vec<char> = marker.chars().collect();
        let mut index = 0usize;
        while index + seq.len() <= chars.len() {
            if skip[index] || chars[index..].len() < seq.len() || !chars[index..].starts_with(&seq) {
                index += 1;
                continue;
            }
            // Look for the closing marker.
            let mut close = index + seq.len();
            let mut found = None;
            while close + seq.len() <= chars.len() {
                if !skip[close] && chars[close..].starts_with(&seq) {
                    found = Some(close);
                    break;
                }
                close += 1;
            }
            match found {
                Some(close) => {
                    for slot in skip.iter_mut().skip(index).take(seq.len()) {
                        *slot = true;
                    }
                    for slot in skip.iter_mut().skip(close).take(seq.len()) {
                        *slot = true;
                    }
                    index = close + seq.len();
                }
                None => index += 1,
            }
        }
    }

    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < chars.len() {
        if skip[index] {
            index += 1;
            continue;
        }
        if chars[index] == '\\' && index + 1 < chars.len() {
            out.push(chars[index + 1]);
            index += 2;
            continue;
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

/// Mark the `[…](…)` tail of links and images so only the label survives.
fn mark_links(chars: &[char], skip: &mut [bool]) {
    let mut index = 0usize;
    while index < chars.len() {
        let bracket = if chars[index] == '!' && index + 1 < chars.len() && chars[index + 1] == '[' {
            index + 1
        } else if chars[index] == '[' {
            index
        } else {
            index += 1;
            continue;
        };

        // The label may hold a pair of its own — `[ [T; N] ](url)` — so the
        // closing bracket is the one that balances the opening one, not the
        // first `]` on the line.
        let mut close = bracket + 1;
        let mut depth = 1usize;
        while close < chars.len() {
            match chars[close] {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            close += 1;
        }
        if close + 1 >= chars.len() || chars[close + 1] != '(' {
            index = bracket + 1;
            continue;
        }

        let mut depth = 0usize;
        let mut end = close + 2;
        let mut matched = false;
        while end < chars.len() {
            match chars[end] {
                '(' => depth += 1,
                ')' if depth == 0 => {
                    matched = true;
                    break;
                }
                ')' => depth -= 1,
                _ => {}
            }
            end += 1;
        }
        if !matched {
            index = bracket + 1;
            continue;
        }

        // Keep the label, drop `[`, `]` and `(dest)`. An image is `![alt](src)`,
        // and the bang is furniture too: the words of it are the alt text.
        if let Some(slot) = skip.get_mut(bracket) {
            *slot = true;
        }
        if bracket == index + 1 {
            skip[index] = true;
        }
        for slot in skip.iter_mut().take(end + 1).skip(close) {
            *slot = true;
        }
        index = end + 1;
    }
}

/// Mark `<u>`, `</u>`, `<mark>`, `<sub>`, `<sup>` and friends.
fn mark_html_tags(chars: &[char], skip: &mut [bool]) {
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] != '<' {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < chars.len() && chars[end] != '>' && end - index < 12 {
            end += 1;
        }
        if end >= chars.len() || chars[end] != '>' {
            index += 1;
            continue;
        }
        let tag: String = chars[index..=end].iter().collect();
        let lower = tag.to_lowercase();
        let known = ["<u>", "</u>", "<mark>", "</mark>", "<sub>", "</sub>", "<sup>", "</sup>", "<br>", "<br/>", "<br />", "<kbd>", "</kbd>"];
        if known.contains(&lower.as_str()) {
            for slot in skip.iter_mut().take(end + 1).skip(index) {
                *slot = true;
            }
            index = end + 1;
        } else {
            index += 1;
        }
    }
}


/// Count words the way a Chinese writer expects: every CJK character is a
/// word, runs of Latin letters and digits count as one.
pub fn count_words(text: &str) -> usize {
    let mut words = 0;
    let mut in_word = false;
    for ch in text.chars() {
        let code = ch as u32;
        let is_cjk = matches!(code, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF);
        if is_cjk {
            words += 1;
            in_word = false;
        } else if ch.is_alphanumeric() {
            if !in_word {
                words += 1;
                in_word = true;
            }
        } else {
            in_word = false;
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_document() {
        let source = "\
# Title

Some **bold** text.

- one
- two

1. first
2. second

> quoted

```rust
fn main() {}
```

---
";
        let blocks = parse(source);
        assert_eq!(blocks.len(), 9);
        assert_eq!(blocks[0].kind, BlockKind::Heading(1));
        assert_eq!(blocks[7].kind, BlockKind::Code(Some("rust".to_string())));
        let back = serialize(&blocks);
        let again = parse(&back);
        assert_eq!(again.len(), blocks.len());
        for (before, after) in blocks.iter().zip(again.iter()) {
            assert_eq!(before.kind, after.kind);
            assert_eq!(before.text, after.text);
        }
    }

    #[test]
    fn parses_todo_before_bullet() {
        let blocks = parse("- [x] done");
        assert_eq!(blocks[0].kind, BlockKind::Todo(true));
        assert_eq!(blocks[0].text, "done");
    }

    #[test]
    fn counts_cjk_words() {
        assert_eq!(count_words("你好 world"), 3);
    }

    #[test]
    fn parses_a_standalone_image() {
        let blocks = parse("![猫](assets/cat.png)");
        assert_eq!(blocks[0].kind, BlockKind::Image);
        let image = parse_image(&blocks[0].text).expect("image");
        assert_eq!(image.src, "assets/cat.png");
        assert_eq!(image.alt, "猫");

        let blocks = parse("![a](<C:/my pics/a b.png> \"标题\")");
        let image = parse_image(&blocks[0].text).expect("image with title");
        assert_eq!(image.src, "C:/my pics/a b.png");
        assert_eq!(image.title.as_deref(), Some("标题"));
        assert_eq!(serialize(&blocks), "![a](<C:/my pics/a b.png> \"标题\")\n");
    }

    #[test]
    fn merges_soft_breaks_into_one_paragraph() {
        let blocks = parse("第一行\n第二行\n\n另一段\n");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text, "第一行\n第二行");
        let back = parse(&serialize(&blocks));
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].text, "第一行\n第二行");
    }

    #[test]
    fn parses_a_pipe_table_into_a_block() {
        let source = "\n前文。\n\n| 名称 | 数量 | 备注 |\n| :--- | ---: | :---: |\n| 苹果 | 3 | 红的 |\n| 梨 | 12 | 黄的 |\n\n后文。\n";
        let blocks = parse(source);
        let kinds: Vec<&BlockKind> = blocks.iter().map(|block| &block.kind).collect();
        assert!(matches!(kinds[1], BlockKind::Table), "table should be its own block");
        assert_eq!(blocks.len(), 3);
        assert!(blocks[1].text.contains("| 苹果 | 3 |"), "rows stay in the text");
        // Serializing keeps the very same Markdown, so the file stays plain.
        let back = serialize(&blocks);
        assert!(back.contains("| 名称 | 数量 | 备注 |"));
        assert_eq!(parse(&back).len(), 3);
    }

    #[test]
    fn reads_header_alignment_and_rows() {
        let table = parse_table(
            "| a | b | c |\n| :-- | :-: | --: |\n| 1 | 2 | 3 |\n| 4 | 5 |",
        )
        .expect("table");
        assert_eq!(table.header, vec!["a", "b", "c"]);
        assert_eq!(
            table.align,
            vec![ColumnAlign::Left, ColumnAlign::Center, ColumnAlign::Right]
        );
        assert_eq!(table.rows.len(), 2);
        // A short row is padded, so every row has a cell per column.
        assert_eq!(table.rows[1], vec!["4", "5", ""]);
    }

    #[test]
    fn a_pipe_inside_a_paragraph_is_not_a_table() {
        let blocks = parse("这一行有一个 | 竖线\n\n---\n");
        assert!(blocks.iter().all(|block| !block.kind.is_table()));
        // Escaped bars stay inside the cell.
        let table = parse_table("| a\\|b | c |\n| --- | --- |\n| x | y |").expect("table");
        assert_eq!(table.header, vec!["a|b", "c"]);
    }

    #[test]
    fn a_table_survives_a_round_trip_through_its_markdown() {
        let source = "| 名称 | 数量 | 备注 |\n| :--- | ---: | :---: |\n| 苹果 | 3 | 红的 |";
        let mut table = parse_table(source).expect("table");
        table.rows[0][1] = "42".to_string();
        let text = table.to_markdown();
        // Left is written without the colons: `:---` means the same thing, and
        // the plain form is what a table looks like when it is typed by hand.
        assert_eq!(
            text,
            "| 名称 | 数量 | 备注 |\n| --- | ---: | :---: |\n| 苹果 | 42 | 红的 |"
        );
        // And the alignment markers come back as they were written.
        let again = parse_table(&text).expect("table again");
        assert_eq!(again.align, table.align);
        assert_eq!(again.rows[0][1], "42");
    }

    #[test]
    fn strips_inline_markup() {
        assert_eq!(strip_inline_markup("**粗体** 和 *斜体*"), "粗体 和 斜体");
        assert_eq!(strip_inline_markup("~~删除~~ `code`"), "删除 code");
        assert_eq!(strip_inline_markup("[链接](https://a.b) 后文"), "链接 后文");
        // A label that carries a bracket pair of its own is still one link.
        assert_eq!(
            strip_inline_markup("[ [T; N] ](https://a.b/array) 是数组"),
            " [T; N]  是数组"
        );
        assert_eq!(strip_inline_markup("拼音 <u>下划线</u>"), "拼音 下划线");
        // Highlights, and an image: the alt text is what a heading says.
        assert_eq!(strip_inline_markup("==高亮== 与 ~~删除~~"), "高亮 与 删除");
        assert_eq!(strip_inline_markup("![架构图](a.png) 说明"), "架构图 说明");
        // Lone markers and identifiers are left alone.
        assert_eq!(strip_inline_markup("snake_case 和 3 * 4"), "snake_case 和 3 * 4");
    }
}
