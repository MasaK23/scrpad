//! The note editor of the terminal UI: long lines wrap at word boundaries, the cursor steps over
//! whole characters (é, emoji, CJK), and each note has its own undo. The text stays in the note;
//! an `Editor` only knows where the cursor is and what came before.

use ratatui::buffer::{Buffer, CellWidth};
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use unicode_segmentation::UnicodeSegmentation;

/// Columns a tab takes: a fixed width, not tab stops, so a line wraps the same wherever it starts.
const TAB: usize = 4;
/// Undo steps kept per note, and the bytes they may hold together (each is a copy of the text).
const UNDO_STEPS: usize = 200;
const UNDO_BYTES: usize = 16 << 20;
/// What a hidden note shows for each character: its lines keep their shape, nothing else does.
const MASK: &str = "•";

pub struct Editor {
    /// Byte offset into the text, on a character boundary.
    cursor: usize,
    /// The first screen row shown.
    scroll: usize,
    /// The column Up and Down keep to across shorter rows.
    goal: Option<usize>,
    /// Width and height of the last draw, and whether it was masked: moving by rows wraps the
    /// text as it was drawn.
    width: usize,
    height: usize,
    mask: bool,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The kind of the last edit: typing on (or deleting on) adds to the same undo step.
    last: Edit,
}

struct Step {
    text: String,
    cursor: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    None,
    Typing,
    Deleting,
}

/// One screen row: `start..end` of the text, and `soft` when the line goes on in the next row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Row {
    start: usize,
    end: usize,
    soft: bool,
}

/// How a grapheme shows: what is drawn, in how many columns. A tab is drawn as spaces.
fn shown(g: &str, mask: bool) -> (&str, usize) {
    if g == "\r" {
        ("", 0)
    } else if mask {
        (MASK, 1)
    } else if g == "\t" {
        (" ", TAB)
    } else if g.contains(char::is_control) {
        ("?", 1)
    } else {
        (g, usize::from(g.cell_width()))
    }
}

/// Splits `text` into screen rows of at most `width` columns, breaking after a space where it
/// can. Spaces at a break hang past the edge, so a row never starts with the space it broke at.
/// A masked text breaks anywhere: where its words end is not for the screen to show.
fn layout(text: &str, width: usize, mask: bool) -> Vec<Row> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut line_start = 0;
    for line in text.split('\n') {
        let (mut start, mut col) = (line_start, 0);
        // Where the row may break: after the last space, and the column there.
        let mut space: Option<(usize, usize)> = None;
        for (i, g) in line.grapheme_indices(true) {
            let at = line_start + i;
            let w = shown(g, mask).1;
            let hangs = g == " " && !mask;
            if col + w > width && col > 0 && !hangs {
                let (cut, cut_col) = match space {
                    Some((offset, c)) if offset > start => (offset, c),
                    _ => (at, col),
                };
                rows.push(Row { start, end: cut, soft: true });
                start = cut;
                col -= cut_col;
                space = None;
            }
            col += w;
            if !mask && (g == " " || g == "\t") {
                space = Some((at + g.len(), col));
            }
        }
        rows.push(Row { start, end: line_start + line.len(), soft: false });
        line_start += line.len() + 1;
    }
    rows
}

/// `text` in rows of at most `width` columns, broken at spaces where it can (for messages).
pub fn wrap(text: &str, width: usize) -> Vec<&str> {
    layout(text, width, false).iter().map(|r| text[r.start..r.end].trim_end()).collect()
}

/// The row the cursor is on: at the end of a wrapped row it is at the start of the next.
fn row_of(rows: &[Row], cursor: usize) -> usize {
    rows.partition_point(|r| r.start <= cursor).saturating_sub(1)
}

/// Columns from the start of `row` to `offset`.
fn column(text: &str, row: Row, offset: usize, mask: bool) -> usize {
    text[row.start..offset.clamp(row.start, row.end)].graphemes(true).map(|g| shown(g, mask).1).sum()
}

/// The offset in `row` closest to column `goal`, never past a wrapped row's last character.
fn offset_at(text: &str, row: Row, goal: usize, mask: bool) -> usize {
    let mut col = 0;
    let mut last = row.start;
    for (i, g) in text[row.start..row.end].grapheme_indices(true) {
        let w = shown(g, mask).1;
        if col + w > goal {
            return row.start + i;
        }
        col += w;
        last = row.start + i;
    }
    if row.soft { last } else { row.end }
}

impl Editor {
    /// An editor for `text`, with the cursor at its end: where a scratchpad note goes on.
    pub fn new(text: &str) -> Self {
        Self {
            cursor: text.len(),
            scroll: 0,
            goal: None,
            width: 80,
            height: 20,
            mask: false,
            undo: Vec::new(),
            redo: Vec::new(),
            last: Edit::None,
        }
    }

    /// The text may have changed under the editor (an append, an Undo from the toast).
    fn clamp(&mut self, text: &str) {
        self.cursor = self.cursor.min(text.len());
        while !text.is_char_boundary(self.cursor) {
            self.cursor -= 1;
        }
    }

    /// Draws the rows of `text` that fit `area`, scrolled to the cursor; where the cursor shows.
    pub fn draw(&mut self, text: &str, mask: bool, area: Rect, buf: &mut Buffer, style: Style) -> Position {
        self.clamp(text);
        (self.width, self.height, self.mask) = (usize::from(area.width).max(1), usize::from(area.height).max(1), mask);
        let rows = layout(text, self.width, mask);
        let current = row_of(&rows, self.cursor);
        self.scroll = self.scroll.min(rows.len().saturating_sub(self.height));
        if current < self.scroll {
            self.scroll = current;
        } else if current >= self.scroll + self.height {
            self.scroll = current + 1 - self.height;
        }
        for (y, row) in (area.y..area.bottom()).zip(&rows[self.scroll..]) {
            let mut x = 0;
            for g in text[row.start..row.end].graphemes(true) {
                let (symbol, w) = shown(g, mask);
                if w == 0 {
                    continue;
                }
                if x + w > self.width {
                    break;
                }
                let cell_x = area.x + x as u16;
                buf[(cell_x, y)].set_symbol(symbol).set_style(style);
                // A wide character covers the next cell; a tab fills its own.
                for i in 1..w as u16 {
                    let cell = &mut buf[(cell_x + i, y)];
                    if symbol == " " {
                        cell.set_symbol(" ").set_style(style);
                    } else {
                        cell.reset();
                    }
                }
                x += w;
            }
        }
        let x = column(text, rows[current], self.cursor, mask).min(self.width - 1);
        Position::new(area.x + x as u16, area.y + (current - self.scroll) as u16)
    }

    // ---- editing (each returns whether the text changed) -----------------------------------

    /// Saves the text as an undo step, unless this edit goes on with the previous one.
    fn checkpoint(&mut self, text: &str, kind: Edit, fresh: bool) {
        if fresh || kind == Edit::None || kind != self.last {
            self.undo.push(Step { text: text.to_owned(), cursor: self.cursor });
            let mut bytes: usize = self.undo.iter().map(|s| s.text.len()).sum();
            while self.undo.len() > UNDO_STEPS || bytes > UNDO_BYTES && self.undo.len() > 1 {
                bytes -= self.undo.remove(0).text.len();
            }
        }
        self.redo.clear();
        self.last = kind;
        self.goal = None;
    }

    /// Inserts typed or pasted text. Line breaks come in as `\n` only.
    pub fn insert(&mut self, text: &mut String, s: &str) -> bool {
        if s.is_empty() {
            return false;
        }
        self.clamp(text);
        // A word typed is one undo step: a space or a line break starts the next.
        let typed = s.chars().count() == 1;
        let kind = if typed { Edit::Typing } else { Edit::None };
        self.checkpoint(text, kind, typed && s.chars().all(char::is_whitespace));
        text.insert_str(self.cursor, s);
        self.cursor += s.len();
        true
    }

    pub fn backspace(&mut self, text: &mut String) -> bool {
        self.clamp(text);
        let Some((prev, _)) = text[..self.cursor].grapheme_indices(true).next_back() else { return false };
        self.checkpoint(text, Edit::Deleting, false);
        text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
        true
    }

    pub fn delete(&mut self, text: &mut String) -> bool {
        self.clamp(text);
        let Some(g) = text[self.cursor..].graphemes(true).next() else { return false };
        let end = self.cursor + g.len();
        self.checkpoint(text, Edit::Deleting, false);
        text.replace_range(self.cursor..end, "");
        true
    }

    pub fn undo(&mut self, text: &mut String) -> bool {
        let Some(step) = self.undo.pop() else { return false };
        self.redo.push(Step { text: std::mem::replace(text, step.text), cursor: self.cursor });
        self.cursor = step.cursor;
        self.last = Edit::None;
        self.clamp(text);
        true
    }

    pub fn redo(&mut self, text: &mut String) -> bool {
        let Some(step) = self.redo.pop() else { return false };
        self.undo.push(Step { text: std::mem::replace(text, step.text), cursor: self.cursor });
        self.cursor = step.cursor;
        self.last = Edit::None;
        self.clamp(text);
        true
    }

    // ---- moving -----------------------------------------------------------------------------

    pub fn move_to(&mut self, text: &str, to: Move) {
        self.clamp(text);
        self.last = Edit::None;
        let c = self.cursor;
        let rows = || layout(text, self.width, self.mask);
        self.cursor = match to {
            Move::Left => text[..c].grapheme_indices(true).next_back().map_or(0, |(i, _)| i),
            Move::Right => text[c..].graphemes(true).next().map_or(c, |g| c + g.len()),
            Move::WordLeft => text[..c].split_word_bound_indices().rev().find(|(_, w)| !w.trim().is_empty()).map_or(0, |(i, _)| i),
            Move::WordRight => {
                text[c..].split_word_bound_indices().find(|(_, w)| !w.trim().is_empty()).map_or(text.len(), |(i, w)| c + i + w.len())
            }
            Move::Home | Move::End => {
                let rows = rows();
                let row = rows[row_of(&rows, c)];
                if to == Move::Home { row.start } else { offset_at(text, row, usize::MAX, self.mask) }
            }
            Move::Up(n) | Move::Down(n) => {
                let rows = rows();
                let current = row_of(&rows, c);
                let goal = *self.goal.get_or_insert_with(|| column(text, rows[current], c, self.mask));
                let n = if n == 0 { self.height.saturating_sub(1).max(1) } else { n };
                let target = if matches!(to, Move::Up(_)) { current.saturating_sub(n) } else { (current + n).min(rows.len() - 1) };
                self.cursor = offset_at(text, rows[target], goal, self.mask);
                return;
            }
            Move::Top => 0,
            Move::Bottom => text.len(),
        };
        self.goal = None;
    }
}

/// Where a cursor key goes. `Up(0)` and `Down(0)` move by a page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Move {
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
    Up(usize),
    Down(usize),
    Top,
    Bottom,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str, width: usize) -> Vec<&str> {
        layout(text, width, false).iter().map(|r| &text[r.start..r.end]).collect()
    }

    #[test]
    fn wraps_at_spaces_and_breaks_long_words() {
        assert_eq!(rows("milk coffee bread", 10), ["milk ", "coffee ", "bread"]);
        assert_eq!(rows("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(rows("a\n\nb", 4), ["a", "", "b"]);
        assert_eq!(rows("", 4), [""]);
        // Wide characters never straddle the edge.
        assert_eq!(rows("日本語です", 5), ["日本", "語で", "す"]);
    }

    #[test]
    fn a_masked_text_wraps_without_showing_its_words() {
        let text = "ab cd";
        let masked: Vec<_> = layout(text, 3, true).iter().map(|r| &text[r.start..r.end]).collect();
        assert_eq!(masked, ["ab ", "cd"]);
    }

    #[test]
    fn the_cursor_steps_over_whole_characters() {
        let mut text = String::from("aé👍🏽日");
        let mut ed = Editor::new(&text);
        ed.move_to(&text, Move::Left);
        assert!(ed.backspace(&mut text));
        assert_eq!(text, "aé日");
        ed.move_to(&text, Move::Left);
        assert!(ed.delete(&mut text));
        assert_eq!(text, "a日");
        ed.insert(&mut text, "ö");
        assert_eq!(text, "aö日");
    }

    #[test]
    fn up_and_down_keep_the_column_across_wrapped_rows() {
        let text = "0123456789\nab\n0123456789";
        let mut ed = Editor::new(text);
        ed.width = 5;
        ed.cursor = 3;
        ed.move_to(text, Move::Down(1));
        assert_eq!(ed.cursor, 8);
        ed.move_to(text, Move::Down(1));
        assert_eq!(ed.cursor, 13, "the end of the short line");
        ed.move_to(text, Move::Down(1));
        assert_eq!(ed.cursor, 17);
    }

    #[test]
    fn a_word_is_one_undo_step() {
        let mut text = String::new();
        let mut ed = Editor::new(&text);
        for c in "hello world".chars() {
            ed.insert(&mut text, &c.to_string());
        }
        assert!(ed.undo(&mut text));
        assert_eq!(text, "hello");
        assert!(ed.undo(&mut text));
        assert_eq!(text, "");
        assert!(ed.redo(&mut text));
        assert_eq!(text, "hello");
    }

    #[test]
    fn draws_wide_characters_and_tabs_in_their_columns() {
        let text = "日\tb";
        let mut ed = Editor::new(text);
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        let cursor = ed.draw(text, false, area, &mut buf, Style::default());
        assert_eq!(buf[(0, 0)].symbol(), "日");
        assert_eq!(buf[(6, 0)].symbol(), "b");
        assert_eq!(cursor, Position::new(7, 0));
    }
}
