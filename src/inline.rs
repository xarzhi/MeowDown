//! Inline Markdown: `**bold**` in, plain text plus styled ranges out.
//!
//! The editor draws a block the same way whether it is focused or not: only the
//! style of the runs changes. That keeps the line height and the text position
//! identical when the caret lands on a paragraph, which is what makes the page
//! stop jumping around while you click through the document.

use std::ops::Range;

pub const BOLD: u8 = 1 << 0;
pub const ITALIC: u8 = 1 << 1;
pub const STRIKE: u8 = 1 << 2;
pub const CODE: u8 = 1 << 3;
pub const LINK: u8 = 1 << 4;
pub const UNDERLINE: u8 = 1 << 5;
pub const MARK: u8 = 1 << 6;
pub const MATH: u8 = 1 << 7;

/// One inline construct that can be opened to show its own Markdown.
///
/// `span` is the whole construct in the source, markers included; `inner` is
/// what sits between them. Both are source offsets, not offsets into the
/// rendered text: the rendered copy drops the markers, so it is only useful for
/// drawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub span: Range<usize>,
    pub inner: Range<usize>,
    pub mask: u8,
}

/// The rendered text plus the style runs that belong to it.
#[derive(Default, Clone)]
pub struct Inline {
    pub text: String,
    /// Byte ranges into `text` with a constant style mask, sorted and disjoint.
    pub spans: Vec<(Range<usize>, u8)>,
    /// Every `[label](url)`: where the label landed in `text`, and the address.
    pub links: Vec<(Range<usize>, String)>,
    /// For every byte of `text`, the byte of the source it was written from.
    /// The markers themselves vanish, so the map answers "which character of
    /// the Markdown is this one" — used by the tests below, and by anything
    /// that has to point from the prose back at the source.
    #[allow(dead_code)]
    pub offsets: Vec<usize>,
    /// Len of the source this was rendered from. Asking for the position *past*
    /// the last character has to answer with the end of the source, not with
    /// the byte the last character started at — otherwise a click past the end
    /// of a line puts the caret before the final character.
    pub source_len: usize,
    /// Every construct whose markers are hidden, innermost last.
    pub elements: Vec<Element>,
    /// Where each construct's content ends in the rendered text, paired with
    /// the source offset just before its closing markers. A caret at one of
    /// these points is still *inside* the construct it belongs to.
    pub inner_ends: Vec<(usize, usize)>,
    /// The markers of every code span, as ranges of `text`.
    ///
    /// They are characters in the line — that is the whole point: a chip is
    /// painted behind the text and can never push the words beside it along, so
    /// the only way to give it padding is to let the markers take up the room.
    /// At rest each one is a blank, which has no ink and therefore disappears
    /// into the chip; `opened` swaps it for its backtick.
    pub marks: Vec<Range<usize>>,
}

impl Inline {
    /// Where in the source the byte `at` of the rendered text came from.
    pub fn source_at(&self, at: usize) -> usize {
        match self.offsets.get(at) {
            Some(offset) => *offset,
            // One past the end of the prose: one past the end of the source.
            None => self.source_len,
        }
    }

    /// The source offset an insertion at the rendered offset `at` belongs at.
    ///
    /// Typing at the end of `**bold**` has to land before the closing markers,
    /// not after them, or the next character would fall out of the run.
    pub fn source_point(&self, at: usize) -> usize {
        if let Some((_, source)) = self.inner_ends.iter().find(|(end, _)| *end == at) {
            return *source;
        }
        self.source_at(at)
    }

    /// Where in the rendered text a caret sitting at `source` belongs.
    ///
    /// The end of a run's content maps to the last rendered character of that
    /// run, not past it: a caret that just typed the final letter is still
    /// inside the run it was typing into.
    pub fn display_of(&self, source: usize) -> usize {
        if let Some((at, _)) = self.inner_ends.iter().find(|(_, point)| *point == source) {
            return *at;
        }
        self.offsets
            .iter()
            .position(|offset| *offset >= source)
            .unwrap_or(self.text.len())
    }

    /// The innermost construct containing `at` in the *source*, if the caret is
    /// strictly inside it — sitting just before `**` or just after it is being
    /// next to the run, not inside it.
    pub fn element_at(&self, at: usize) -> Option<&Element> {
        self.elements_at(at).into_iter().next()
    }

    /// Every construct containing `at` in the source, innermost first.
    ///
    /// One line of Markdown can nest: `[`Ctrl+T`](url)` is a chip inside a
    /// link, and a caret in the chip is inside both. Opening only the innermost
    /// one would show the backticks but keep the address hidden — the half of
    /// the source that cannot be reached any other way.
    pub fn elements_at(&self, at: usize) -> Vec<&Element> {
        let mut found: Vec<&Element> = self
            .elements
            .iter()
            .filter(|element| element.span.start < at && at < element.span.end)
            .collect();
        found.sort_by_key(|element| element.span.end - element.span.start);
        found
    }

    /// Where the rendered text starts and ends for a construct's content.
    pub fn rendered_range(&self, element: &Element) -> Option<(usize, usize)> {
        let start = self
            .offsets
            .iter()
            .position(|offset| *offset >= element.inner.start)?;
        let end = self
            .offsets
            .iter()
            .rposition(|offset| *offset < element.inner.end)
            .map(|at| at + 1)
            .unwrap_or(start);
        Some((start, end.max(start)))
    }

    /// The same view with one construct's markers put back.
    ///
    /// Everything else keeps its rendered form, so opening a `code` chip inside
    /// a bold run does not spill the bold markers too: only the run the caret
    /// is sitting in shows its syntax.
    pub fn opened(&self, element: &Element, source: &str) -> Option<Inline> {        let (start, end) = self.rendered_range(element)?;
        // A chip keeps its markers in the text as blanks, so there is nothing to
        // put back: opening the run swaps each blank for its backtick, one byte
        // for one byte, and nothing on the line moves.
        if element.mask == CODE {
            if let Some(pair) = self.marker_pair(start, end) {
                let mut opened = self.clone();
                opened.text.replace_range(pair.start..pair.start + 1, "`");
                opened.text.replace_range(pair.end - 1..pair.end, "`");
                return Some(opened);
            }
        }
        let open = source.get(element.span.start..element.inner.start)?;
        let close = source.get(element.inner.end..element.span.end)?;
        if open.is_empty() && close.is_empty() {
            return None;
        }

        let mut text = String::with_capacity(self.text.len() + open.len() + close.len());
        text.push_str(&self.text[..start]);
        text.push_str(open);
        text.push_str(&self.text[start..end]);
        text.push_str(close);
        text.push_str(&self.text[end..]);

        let mut offsets = Vec::with_capacity(text.len());
        offsets.extend_from_slice(&self.offsets[..start]);
        // A marker byte maps to its own place in the source; it has no rendered
        // copy to copy over.
        offsets.extend(element.span.start..element.inner.start);
        offsets.extend_from_slice(&self.offsets[start..end]);
        offsets.extend(element.inner.end..element.span.end);
        offsets.extend_from_slice(&self.offsets[end..]);

        // A position at the end of the content belongs *inside* it: it is the
        // spot the closing marker is about to be written at, not the first
        // character of whatever follows the construct. A range needs both
        // halves of that rule, or a chip that ends where its label ends would
        // swallow the `](url)` that gets written after it.
        let from = |at: usize| {
            if at >= end {
                at + open.len() + close.len()
            } else if at >= start {
                at + open.len()
            } else {
                at
            }
        };
        let to = |at: usize| {
            if at > end {
                at + open.len() + close.len()
            } else if at > start {
                at + open.len()
            } else {
                at
            }
        };
        let spans = self
            .spans
            .iter()
            .filter_map(|(range, mask)| {
                let from = from(range.start);
                let to = to(range.end);
                (from < to).then_some((from..to, *mask))
            })
            .collect();
        let links = self
            .links
            .iter()
            .map(|(range, url)| (from(range.start)..to(range.end), url.clone()))
            .collect();
        let inner_ends = self
            .inner_ends
            .iter()
            .map(|(at, source)| (from(*at), *source))
            .collect();
        let marks = self
            .marks
            .iter()
            .map(|mark| from(mark.start)..to(mark.end))
            .collect();

        Some(Inline {
            text,
            spans,
            links,
            offsets,
            source_len: self.source_len,
            elements: self.elements.clone(),
            inner_ends,
            marks,
        })
    }

    /// The view a caret at `at` asks for: the construct it sits in opens, and
    /// so does any link around it.
    ///
    /// A link is the one construct whose source lives somewhere else on the
    /// line: `[`Ctrl+T`](https://…)` hides the address inside `](…)`, and there
    /// is no other place to put a caret to edit it. So a caret inside a chip in
    /// a link label opens both — the backticks *and* the brackets with the
    /// address — while a `**` around the same chip stays shut, because a bold
    /// run's markers are not hiding anything the chip does not already show.
    pub fn opened_at(&self, at: usize, source: &str) -> Option<Inline> {
        let chain: Vec<Element> = self.elements_at(at).into_iter().cloned().collect();
        let mut view = self.clone();
        let mut opened = false;
        for (depth, element) in chain.iter().enumerate() {
            if depth > 0 && element.mask & LINK == 0 {
                continue;
            }
            if let Some(next) = view.opened(element, source) {
                view = next;
                opened = true;
            }
        }
        opened.then_some(view)
    }

    /// The two marks that hug a run's content, as one range.
    fn marker_pair(&self, start: usize, end: usize) -> Option<Range<usize>> {
        let open = self.marks.iter().find(|mark| mark.end == start)?;
        let close = self.marks.iter().find(|mark| mark.start == end)?;
        Some(open.start..close.end)
    }

    /// The chip whose backticks are on show, if one is.
    ///
    /// Opening a chip is the only thing in the scanner that writes a backtick
    /// into the line: at rest the two marks are blanks, and no other construct
    /// leaves a backtick behind. So the bytes say which chip is being edited —
    /// the one whose fill has to be taken away while its source is on show.
    pub fn opened_chip(&self) -> Option<Range<usize>> {
        self.marks
            .chunks_exact(2)
            .find(|pair| self.text.as_bytes().get(pair[0].start) == Some(&b'`'))
            .map(|pair| pair[0].start..pair[1].end)
    }
}

/// Render one line of Markdown into plain text and style runs.
pub fn render(source: &str) -> Inline {
    let raw = scan(source);
    Inline {
        spans: coalesce(&raw.masks),
        text: raw.text,
        links: raw.links,
        offsets: raw.offsets,
        source_len: source.len(),
        elements: raw.elements,
        inner_ends: raw.inner_ends,
        marks: raw.marks,
    }
}

/// What the scanner produces: the text, one style mask and one source offset
/// per byte of it, and the links it came across.
struct Raw {
    text: String,
    masks: Vec<u8>,
    offsets: Vec<usize>,
    links: Vec<(Range<usize>, String)>,
    elements: Vec<Element>,
    inner_ends: Vec<(usize, usize)>,
    marks: Vec<Range<usize>>,
}

fn scan(source: &str) -> Raw {
    let chars: Vec<char> = source.chars().collect();
    // Byte offset of every char in the source, plus one past the end, so an
    // emitted char can be traced back to the bytes the user actually typed.
    let mut bytes_at: Vec<usize> = Vec::with_capacity(chars.len() + 1);
    let mut cursor = 0usize;
    for ch in &chars {
        bytes_at.push(cursor);
        cursor += ch.len_utf8();
    }
    bytes_at.push(cursor);

    let mut out = String::with_capacity(source.len());
    // One style mask per byte of `out`, coalesced into `spans` at the end.
    let mut masks: Vec<u8> = Vec::with_capacity(source.len());
    let mut offsets: Vec<usize> = Vec::with_capacity(source.len());
    let mut links: Vec<(Range<usize>, String)> = Vec::new();
    let mut elements: Vec<Element> = Vec::new();
    let mut inner_ends: Vec<(usize, usize)> = Vec::new();
    let mut marks: Vec<Range<usize>> = Vec::new();
    // Open formatting markers: (marker id, style bit, where the marker starts,
    // where the content after it starts).
    let mut stack: Vec<(u8, u8, usize, usize)> = Vec::new();

    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];

        // A backslash escapes the next character, which is how Markdown lets
        // you type a literal `*`.
        if ch == '\\' && index + 1 < chars.len() {
            let escaped = chars[index + 1];
            push(&mut out, &mut masks, &mut offsets, &stack, escaped, bytes_at[index + 1]);
            index += 2;
            continue;
        }

        // Code spans swallow their content verbatim.
        if ch == '`' {
            if let Some(end) = find(&chars, index + 1, '`') {
                let mut code: Vec<u8> = stack.iter().map(|(_, bit, ..)| *bit).collect();
                code.push(CODE);
                // A chip is painted *behind* the line; it can never move the
                // words beside it. So the two things a chip needs are made of
                // text: the backticks stay in, and they supply the padding
                // (at rest they are painted in the chip's own colour, which
                // makes them invisible inside it), while a word hard up against
                // the chip gets one space of real air, which is the margin.
                let outside = stack_mask(&stack);
                if hard_up(&chars, index as isize - 1) {
                    push_masked(
                        &mut out,
                        &mut masks,
                        &mut offsets,
                        outside,
                        ' ',
                        bytes_at[index],
                    );
                }
                let open = out.len();
                // A blank, not the backtick: a character painted in a colour
                // that matches the chip would still show as soon as something
                // else is behind it — a selection, say. A blank has no ink at
                // all, so it disappears wherever it is drawn. `opened` swaps it
                // back for the backtick, one byte for one byte.
                push_with(&mut out, &mut masks, &mut offsets, &code, ' ', bytes_at[index]);
                marks.push(open..out.len());
                for at in index + 1..end {
                    push_with(&mut out, &mut masks, &mut offsets, &code, chars[at], bytes_at[at]);
                }
                let close = out.len();
                push_with(&mut out, &mut masks, &mut offsets, &code, ' ', bytes_at[end]);
                marks.push(close..out.len());
                if hard_up(&chars, end as isize + 1) {
                    push_masked(
                        &mut out,
                        &mut masks,
                        &mut offsets,
                        outside,
                        ' ',
                        bytes_at[end],
                    );
                }
                elements.push(Element {
                    span: bytes_at[index]..bytes_at[end + 1],
                    inner: bytes_at[index + 1]..bytes_at[end],
                    mask: CODE,
                });
                // The content ends at the closing backtick, which is a real
                // character now: a caret just before it is still inside.
                inner_ends.push((close, bytes_at[end]));
                index = end + 1;
                continue;
            }
        }

        // `[label](url)`, and the `![alt](url)` form that only differs by the
        // bang (an image is a block of its own, so here it is just its label).
        if ch == '[' || (ch == '!' && chars.get(index + 1) == Some(&'[')) {
            let open = if ch == '!' { index + 2 } else { index + 1 };
            if let Some((label_end, url, after)) = parse_link(&chars, open) {
                let label: String = chars[open..label_end].iter().collect();
                // Half a link is not a link yet. `[](…)` and `[文字]()` are what
                // the line looks like while it is being typed — and both have a
                // part that would otherwise be hidden for good: an address with
                // no label to sit under, or brackets with nothing inside. So
                // the whole thing stays literal until there are two ends of it,
                // which is also what lets a caret be put between them.
                if label.is_empty() || url.is_empty() {
                    push(&mut out, &mut masks, &mut offsets, &stack, ch, bytes_at[index]);
                    index += 1;
                    continue;
                }
                let start = out.len();
                // The label is Markdown of its own: `[`Ctrl+T`](…)` keeps its
                // code chip, `[**粗**](…)` stays bold. The link bit is or-ed on
                // top, so the run reads as one link whichever styles it carries.
                let inner = scan(&label);
                let base = stack_mask(&stack) | LINK;
                push_run(
                    &mut out,
                    &mut masks,
                    &mut offsets,
                    &inner,
                    base,
                    bytes_at[open],
                );
                if !url.is_empty() && out.len() > start {
                    links.push((start..out.len(), url));
                }
                elements.push(Element {
                    span: bytes_at[index]..bytes_at[after],
                    inner: bytes_at[open]..bytes_at[label_end],
                    mask: LINK,
                });
                inner_ends.push((out.len(), bytes_at[label_end]));
                // A label is Markdown of its own, so the runs inside it are
                // constructs too — shifted to where the label sits here.
                let shift = bytes_at[open];
                for element in inner.elements {
                    elements.push(Element {
                        span: element.span.start + shift..element.span.end + shift,
                        inner: element.inner.start + shift..element.inner.end + shift,
                        mask: element.mask,
                    });
                }
                for mark in inner.marks {
                    marks.push(start + mark.start..start + mark.end);
                }
                index = after;
                continue;
            }
        }

        if ch == '$' {
            if let Some(end) = find(&chars, index + 1, '$') {
                if end > index + 1 {
                    let mut math: Vec<u8> = stack.iter().map(|(_, bit, ..)| *bit).collect();
                    math.push(MATH);
                    for at in index..=end {
                        push_with(&mut out, &mut masks, &mut offsets, &math, chars[at], bytes_at[at]);
                    }
                    elements.push(Element {
                        span: bytes_at[index]..bytes_at[end + 1],
                        inner: bytes_at[index + 1]..bytes_at[end],
                        mask: MATH,
                    });
                    inner_ends.push((out.len(), bytes_at[end]));
                    index = end + 1;
                    continue;
                }
            }
        }

        if ch == '<' {
            if let Some(end) = find(&chars, index, '>') {
                let tag: String = chars[index..=end].iter().collect::<String>().to_lowercase();
                match tag.as_str() {
                    "<u>" => {
                        stack.push((b'u', UNDERLINE, bytes_at[index], bytes_at[end + 1]));
                        index = end + 1;
                        continue;
                    }
                    "</u>" => {
                        if let Some((_, bit, open_start, content_start)) = take(&mut stack, b'u') {
                            elements.push(Element {
                                span: open_start..bytes_at[end + 1],
                                inner: content_start..bytes_at[index],
                                mask: bit,
                            });
                            inner_ends.push((out.len(), bytes_at[index]));
                        }
                        index = end + 1;
                        continue;
                    }
                    "<mark>" => {
                        stack.push((b'm', MARK, bytes_at[index], bytes_at[end + 1]));
                        index = end + 1;
                        continue;
                    }
                    "</mark>" => {
                        if let Some((_, bit, open_start, content_start)) = take(&mut stack, b'm') {
                            elements.push(Element {
                                span: open_start..bytes_at[end + 1],
                                inner: content_start..bytes_at[index],
                                mask: bit,
                            });
                            inner_ends.push((out.len(), bytes_at[index]));
                        }
                        index = end + 1;
                        continue;
                    }
                    "<br>" | "<br/>" | "<br />" => {
                        push(&mut out, &mut masks, &mut offsets, &stack, ' ', bytes_at[index]);
                        index = end + 1;
                        continue;
                    }
                    _ => {}
                }
            }
        }

        // Emphasis markers, longest first so `**` wins over `*`.
        const MARKERS: &[(&str, u8, u8)] = &[
            ("***", b'a', BOLD | ITALIC),
            ("___", b'b', BOLD | ITALIC),
            ("**", b'c', BOLD),
            ("__", b'd', BOLD),
            ("~~", b'e', STRIKE),
            ("==", b'f', MARK),
            ("*", b'g', ITALIC),
            ("_", b'h', ITALIC),
        ];
        let mut matched = false;
        for (marker, id, bit) in MARKERS {
            if !starts_with(&chars, index, marker) {
                continue;
            }
            if let Some(position) = stack.iter().rposition(|(open, ..)| open == id) {
                // A marker that is already open closes the run, whatever sits
                // next to it: `*中文*` ends with a `*` that follows a letter,
                // and the boundary rule would otherwise print it literally.
                let (_, bit, open_start, content_start) = stack.remove(position);
                elements.push(Element {
                    span: open_start..bytes_at[index + marker.len()],
                    inner: content_start..bytes_at[index],
                    mask: bit,
                });
                inner_ends.push((out.len(), bytes_at[index]));
            } else if (*marker == "*" || *marker == "_")
                && !emphasis_boundary(&chars, index, marker.len())
            {
                // `snake_case` must not turn into italics, so an underscore or
                // a star in the middle of a word can only open a run when it
                // starts one.
                continue;
            } else if has_closing(&chars, index + marker.len(), marker) {
                stack.push((*id, *bit, bytes_at[index], bytes_at[index + marker.len()]));
            } else {
                continue;
            }
            index += marker.len();
            matched = true;
            break;
        }
        if matched {
            continue;
        }

        push(&mut out, &mut masks, &mut offsets, &stack, ch, bytes_at[index]);
        index += 1;
    }

    Raw {
        text: out,
        masks,
        offsets,
        links,
        elements,
        inner_ends,
        marks,
    }
}

/// Whether a word sits against the chip on that side, so a space of real text
/// has to be spent there. A blank already in the source does the job, and so
/// does the start or the end of the line — only a letter or a digit leaves the
/// chip touching something.
fn hard_up(chars: &[char], at: isize) -> bool {
    usize::try_from(at)
        .ok()
        .and_then(|at| chars.get(at))
        .is_some_and(|ch| ch.is_alphanumeric())
}

/// The style bits every open marker contributes.
fn stack_mask(stack: &[(u8, u8, usize, usize)]) -> u8 {
    stack.iter().fold(0u8, |acc, (_, bit, ..)| acc | *bit)
}

/// Take one open marker off the stack, whatever sits above it.
fn take(stack: &mut Vec<(u8, u8, usize, usize)>, id: u8) -> Option<(u8, u8, usize, usize)> {
    let position = stack.iter().rposition(|(open, ..)| *open == id)?;
    Some(stack.remove(position))
}

fn push(
    out: &mut String,
    masks: &mut Vec<u8>,
    offsets: &mut Vec<usize>,
    stack: &[(u8, u8, usize, usize)],
    ch: char,
    from: usize,
) {
    push_masked(out, masks, offsets, stack_mask(stack), ch, from);
}

fn push_with(
    out: &mut String,
    masks: &mut Vec<u8>,
    offsets: &mut Vec<usize>,
    bits: &[u8],
    ch: char,
    from: usize,
) {
    let mask = bits.iter().fold(0u8, |acc, bit| acc | *bit);
    push_masked(out, masks, offsets, mask, ch, from);
}

fn push_masked(
    out: &mut String,
    masks: &mut Vec<u8>,
    offsets: &mut Vec<usize>,
    mask: u8,
    ch: char,
    from: usize,
) {
    out.push(ch);
    for _ in 0..ch.len_utf8() {
        masks.push(mask);
        offsets.push(from);
    }
}

/// Append a run that was scanned on its own, re-masked with `base` on top of
/// whatever styles it already carries. `shift` is the byte offset the run's
/// own source started at, so its offsets line up with the outer document.
fn push_run(
    out: &mut String,
    masks: &mut Vec<u8>,
    offsets: &mut Vec<usize>,
    inner: &Raw,
    base: u8,
    shift: usize,
) {
    let mut offset = 0;
    for ch in inner.text.chars() {
        let extra = inner.masks.get(offset).copied().unwrap_or(0);
        let from = inner.offsets.get(offset).copied().unwrap_or(0) + shift;
        push_masked(out, masks, offsets, base | extra, ch, from);
        offset += ch.len_utf8();
    }
}

/// Merge equal neighbouring masks into disjoint ranges.
fn coalesce(masks: &[u8]) -> Vec<(Range<usize>, u8)> {
    let mut spans: Vec<(Range<usize>, u8)> = Vec::new();
    for (index, mask) in masks.iter().enumerate() {
        if *mask == 0 {
            continue;
        }
        match spans.last_mut() {
            Some((range, previous)) if *previous == *mask && range.end == index => {
                range.end = index + 1;
            }
            _ => spans.push((index..index + 1, *mask)),
        }
    }
    spans
}

fn starts_with(chars: &[char], at: usize, pattern: &str) -> bool {
    let mut cursor = at;
    for expected in pattern.chars() {
        if chars.get(cursor) != Some(&expected) {
            return false;
        }
        cursor += 1;
    }
    true
}

fn find(chars: &[char], from: usize, needle: char) -> Option<usize> {
    (from..chars.len()).find(|index| chars[*index] == needle)
}

fn has_closing(chars: &[char], from: usize, marker: &str) -> bool {
    let mut cursor = from;
    while cursor < chars.len() {
        if starts_with(chars, cursor, marker) {
            return true;
        }
        cursor += 1;
    }
    false
}

/// `*` and `_` only open emphasis when the character before is a boundary.
fn emphasis_boundary(chars: &[char], at: usize, width: usize) -> bool {
    let before = if at == 0 { None } else { chars.get(at - 1) };
    let after = chars.get(at + width);
    if before.is_some_and(|ch| ch.is_alphanumeric()) && !starts_with(chars, at, "__") {
        return false;
    }
    !after.is_some_and(|ch| ch.is_whitespace())
}

/// Parse `label](url)` starting just after the `[`; returns where the label
/// ends, the address, and the index just past the closing `)`.
fn parse_link(chars: &[char], from: usize) -> Option<(usize, String, usize)> {
    let label_end = find(chars, from, ']')?;
    if chars.get(label_end + 1) != Some(&'(') {
        return None;
    }
    let url_end = find(chars, label_end + 2, ')')?;
    let raw: String = chars[label_end + 2..url_end].iter().collect();
    // `[文字](地址 "标题")` — the optional title is not part of the address.
    let url = match raw.find(char::is_whitespace) {
        Some(cut) => raw[..cut].to_string(),
        None => raw,
    };
    Some((label_end, url.trim().to_string(), url_end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_bold_markers() {
        let inline = render("这是 **粗体** 文本");
        assert_eq!(inline.text, "这是 粗体 文本");
        assert_eq!(inline.spans.len(), 1);
        let (range, mask) = &inline.spans[0];
        assert_eq!(&inline.text[range.clone()], "粗体");
        assert_eq!(*mask, BOLD);
    }

    #[test]
    fn emphasis_closes_even_after_a_cjk_letter() {
        let inline = render("*斜体* 与 **加粗**");
        assert_eq!(inline.text, "斜体 与 加粗");
        assert_eq!(inline.spans[0].1, ITALIC);
        assert_eq!(&inline.text[inline.spans[0].0.clone()], "斜体");
        assert_eq!(inline.spans[1].1, BOLD);
    }

    #[test]
    fn keeps_identifiers_intact() {
        let inline = render("snake_case_name 和 3 * 4");
        assert_eq!(inline.text, "snake_case_name 和 3 * 4");
        assert!(inline.spans.is_empty());
    }

    #[test]
    fn links_keep_their_label() {
        let inline = render("见 [文档](https://a.b) 结束");
        assert_eq!(inline.text, "见 文档 结束");
        assert_eq!(inline.spans[0].1, LINK);
    }

    #[test]
    fn links_remember_where_they_point() {
        let inline = render("见 [文档](https://a.b) 结束");
        assert_eq!(inline.links.len(), 1);
        let (range, url) = &inline.links[0];
        assert_eq!(&inline.text[range.clone()], "文档");
        assert_eq!(url, "https://a.b");
    }

    #[test]
    fn a_link_label_may_carry_inline_code() {
        let inline = render("按 [`Ctrl+T`](https://a.b) 插入");
        assert_eq!(inline.text, "按  Ctrl+T  插入");
        let (range, mask) = &inline.spans[0];
        // The backticks stay in the line: they are the chip's padding.
        // The blanks around it are the chip's padding; it keeps the whole span.
        assert_eq!(&inline.text[range.clone()], " Ctrl+T ");
        // Both bits: it is a code chip that also opens a link.
        assert_eq!(*mask, CODE | LINK);
        assert_eq!(inline.links[0].1, "https://a.b");
    }

    #[test]
    fn a_link_url_may_carry_a_title() {
        let inline = render("[文档](https://a.b \"标题\")");
        assert_eq!(inline.links[0].1, "https://a.b");
        assert_eq!(inline.text, "文档");
    }

    #[test]
    fn offsets_walk_back_to_the_source() {
        let source = "这是 **粗体** 文本";
        let inline = render(source);
        assert_eq!(inline.text, "这是 粗体 文本");
        let at = inline.text.find('粗').unwrap();
        assert_eq!(inline.source_at(at), "这是 **".len());

        // A link label sits inside `[…]`, after the bracket was dropped.
        let source = "见 [文档](https://a.b)";
        let inline = render(source);
        let at = inline.text.find('文').unwrap();
        assert_eq!(inline.source_at(at), "见 [".len());

        // Code spans drop their backticks too.
        let source = "用 `a * b` 算";
        let inline = render(source);
        let at = inline.text.find('a').unwrap();
        assert_eq!(inline.source_at(at), "用 `".len());
    }

    #[test]
    fn the_end_of_the_prose_is_the_end_of_the_source() {
        let source = "见 [文档](https://a.b) 结束";
        let inline = render(source);
        assert_eq!(inline.source_at(inline.text.len()), source.len());
    }

    #[test]
    fn a_run_knows_where_its_markers_are() {
        let source = "这是 **粗体** 文字";
        let inline = render(source);
        let at = inline.source_at(inline.text.find('粗').unwrap());
        let element = inline.element_at(at).expect("the caret is inside the run");
        assert_eq!(&source[element.span.clone()], "**粗体**");
        assert_eq!(&source[element.inner.clone()], "粗体");
    }

    #[test]
    fn opening_a_run_puts_its_markers_back() {
        let source = "这是 **粗体** 文字";
        let inline = render(source);
        let at = inline.source_at(inline.text.find('粗').unwrap());
        let element = inline.element_at(at).unwrap();
        let opened = inline.opened(element, source).unwrap();
        assert_eq!(opened.text, source);
        // The map still walks back to the source, markers included.
        assert_eq!(opened.source_at(0), 0);
        let bold = opened.text.find('粗').unwrap();
        assert_eq!(opened.source_point(bold), source.find('粗').unwrap());
    }

    #[test]
    fn typing_at_the_end_of_a_run_stays_inside_it() {
        let source = "见 **粗体**。";
        let inline = render(source);
        let element = inline.element_at(source.find('粗').unwrap()).unwrap();
        assert_eq!(inline.source_point(inline.text.find('体').unwrap() + '体'.len_utf8()), element.inner.end);
    }

    #[test]
    fn the_innermost_run_is_the_one_that_opens() {
        let source = "**粗 `c` 体**";
        let inline = render(source);
        let at = inline.source_at(inline.text.find('c').unwrap());
        let element = inline.element_at(at).unwrap();
        assert_eq!(element.mask, CODE);
        let opened = inline.opened(element, source).unwrap();
        assert_eq!(opened.text, "粗 `c` 体");
    }

    #[test]
    fn a_link_opens_its_brackets_and_address() {
        let source = "见 [文档](https://a.b) 结束";
        let inline = render(source);
        let at = inline.source_at(inline.text.find('文').unwrap());
        let element = inline.element_at(at).unwrap();
        assert_eq!(element.mask, LINK);
        let opened = inline.opened(element, source).unwrap();
        assert_eq!(opened.text, source);
    }

    #[test]
    fn the_caret_next_to_a_run_does_not_open_it() {
        let source = "**粗体** 后";
        let inline = render(source);
        assert!(inline.element_at(0).is_none());
        assert!(inline.element_at(source.len()).is_none());
        // Between the two opening markers is already inside the run.
        assert!(inline.element_at(1).is_some());
        assert!(inline.element_at(source.find('粗').unwrap()).is_some());
    }

    /// Every position the prose can ask about has to answer with a byte the
    /// Markdown can actually be cut at. A position in the middle of a Chinese
    /// character has no counterpart in the source, and slicing the source there
    /// panics — which is exactly what happened when an edit was translated back
    /// from the prose by rounding in the wrong direction.
    #[test]
    fn a_source_position_is_always_a_character_boundary() {
        let source = "前 **粗体** 与 `代码` 后";
        let inline = render(source);
        for at in 0..=inline.text.len() {
            let cut = inline.source_at(at);
            assert!(
                source.is_char_boundary(cut),
                "{at} maps to {cut}, which splits a character"
            );
        }
        assert_eq!(inline.source_at(inline.text.len()), source.len());
    }

    /// A run knows which slice of the prose it occupies, which is what lets an
    /// edit that removed exactly that slice take the markers with it.
    #[test]
    fn a_run_reports_the_rendered_range_it_occupies() {
        let inline = render("前**粗**后");
        let element = &inline.elements[0];
        let (start, end) = inline.rendered_range(element).unwrap();
        assert_eq!(&inline.text[start..end], "粗");
    }

    #[test]
    fn code_spans_are_verbatim() {
        let inline = render("用 `a * b` 计算");
        assert_eq!(inline.text, "用  a * b  计算");
        let (range, mask) = &inline.spans[0];
        assert_eq!(&inline.text[range.clone()], " a * b ");
        assert_eq!(*mask, CODE);
    }

    /// A chip cannot push the words beside it along — it is painted behind the
    /// line — so its padding is made of text: the two markers, kept as blanks
    /// so they have no ink to show and hold the chip open all the same.
    #[test]
    fn a_chip_keeps_the_markers_that_hold_its_padding_open() {
        let inline = render("用 `a` 算");
        assert_eq!(inline.marks.len(), 2);
        assert_eq!(&inline.text[inline.marks[0].clone()], " ");
        assert_eq!(&inline.text[inline.marks[1].clone()], " ");
        // Between them sits the content, and nothing else.
        assert_eq!(
            &inline.text[inline.marks[0].end..inline.marks[1].start],
            "a"
        );
    }

    /// A word hard against a chip would touch it. One space of real text is the
    /// only thing that can hold it off.
    #[test]
    fn a_chip_makes_room_where_a_word_sits_against_it() {
        // One blank of padding, plus one space of margin.
        assert_eq!(render("有`码`夹").text, "有  码  夹");
        // A blank already in the source is room enough — same line as above.
        assert_eq!(render("有 `码` 夹").text, "有  码  夹");
        // And the ends of the line need none.
        assert_eq!(render("`码`开头").text, " 码  开头");
        assert_eq!(render("结尾`码`").text, "结尾  码 ");
    }

    /// Opening a chip shows its backticks; it does not have to put them back,
    /// because the markers have been in the line all along — as blanks.
    #[test]
    fn opening_a_chip_only_stops_hiding_its_markers() {
        let source = "用 `a` 算";
        let inline = render(source);
        let element = inline.element_at(source.find('a').unwrap()).unwrap();
        assert_eq!(element.mask, CODE);
        // At rest the markers are blanks: the space that was already in the
        // source, plus one blank of padding on each side.
        assert_eq!(inline.text, "用  a  算");
        let opened = inline.opened(element, source).unwrap();
        assert_eq!(opened.text, "用 `a` 算");
        // Same length: the swap moves nothing on the line.
        assert_eq!(opened.text.len(), inline.text.len());
    }

    /// Opening one chip must not put a fill back on the other: only the run the
    /// caret is in is the one being edited.
    #[test]
    fn only_the_chip_that_is_open_has_its_backticks_out() {
        let source = "用 `a` 与 `b` 算";
        let inline = render(source);
        assert!(inline.opened_chip().is_none());
        let element = inline.element_at(source.find('b').unwrap()).unwrap();
        let opened = inline.opened(element, source).unwrap();
        let chip = opened.opened_chip().expect("the second chip is open");
        assert_eq!(&opened.text[chip], "`b`");
    }

    /// A label is Markdown of its own, so a chip inside a link is a construct
    /// inside a construct. The caret sits in both, and both have source the
    /// other does not show: the backticks around the chip, the brackets and the
    /// address around the label. Opening only the innermost would leave the
    /// address — the part that has to be edited — out of reach.
    #[test]
    fn a_chip_inside_a_link_opens_the_chip_and_the_link() {
        let source = "按 [`Ctrl+T`](https://a.b) 插入";
        let inline = render(source);
        assert_eq!(inline.text, "按  Ctrl+T  插入");
        let at = source.find('C').unwrap() + 1;
        assert_eq!(inline.elements_at(at).len(), 2, "chip inside link");
        let opened = inline.opened_at(at, source).expect("both constructs open");
        assert_eq!(opened.text, source);
        let chip = opened.opened_chip().expect("the chip is open too");
        assert_eq!(&opened.text[chip], "`Ctrl+T`");
    }

    /// A `**` around the same chip is not something the chip is hiding, so it
    /// stays shut: the line shows the source of what is being edited, not the
    /// source of everything that encloses it.
    #[test]
    fn a_bold_run_around_a_chip_stays_shut() {
        let source = "**粗 `c` 体**";
        let inline = render(source);
        let at = source.find('c').unwrap();
        let opened = inline.opened_at(at, source).unwrap();
        assert_eq!(opened.text, "粗 `c` 体");
    }
}


