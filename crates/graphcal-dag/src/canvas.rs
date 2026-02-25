//! A 2D character grid for rendering DAGs with Unicode box-drawing characters.
//!
//! The canvas supports smart merging of overlapping box-drawing characters
//! using a direction-bitmask approach.

/// Direction bitmask for box-drawing character merging.
const UP: u8 = 0b0001;
const DOWN: u8 = 0b0010;
const LEFT: u8 = 0b0100;
const RIGHT: u8 = 0b1000;

/// A 2D character grid.
pub struct Canvas {
    cells: Vec<Vec<char>>,
    width: usize,
    height: usize,
}

impl Canvas {
    /// Create a new canvas filled with spaces.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            cells: vec![vec![' '; width]; height],
            width,
            height,
        }
    }

    /// Put a character at (row, col), merging box-drawing characters if both
    /// the existing and new characters are box-drawing.
    pub fn put(&mut self, row: usize, col: usize, ch: char) {
        if row >= self.height || col >= self.width {
            return;
        }
        let existing = self.cells[row][col];
        self.cells[row][col] = merge_box_chars(existing, ch);
    }

    /// Put a character at (row, col), overwriting whatever is there (no merge).
    pub fn put_overwrite(&mut self, row: usize, col: usize, ch: char) {
        if row < self.height && col < self.width {
            self.cells[row][col] = ch;
        }
    }

    /// Write a string starting at (row, col). Non-box characters overwrite.
    pub fn put_str(&mut self, row: usize, col: usize, s: &str) {
        for (i, ch) in s.chars().enumerate() {
            let c = col + i;
            if c >= self.width {
                break;
            }
            if row < self.height {
                self.cells[row][c] = ch;
            }
        }
    }

    /// Draw a vertical line from `row_start` to `row_end` (inclusive) at `col`.
    pub fn vline(&mut self, col: usize, row_start: usize, row_end: usize) {
        let (lo, hi) = if row_start <= row_end {
            (row_start, row_end)
        } else {
            (row_end, row_start)
        };
        for r in lo..=hi {
            self.put(r, col, '│');
        }
    }

    /// Render the canvas to a string, trimming trailing whitespace per line
    /// and trailing empty lines.
    pub fn to_string_trimmed(&self) -> String {
        let mut lines: Vec<String> = self
            .cells
            .iter()
            .map(|row| {
                let s: String = row.iter().collect();
                s.trim_end().to_string()
            })
            .collect();

        // Remove trailing empty lines.
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }

        lines.join("\n")
    }
}

/// Merge two characters, combining box-drawing directions when both are
/// box-drawing characters. If either is a space, the other wins.
/// Non-box characters overwrite.
const fn merge_box_chars(existing: char, new: char) -> char {
    if existing == ' ' {
        return new;
    }
    if new == ' ' {
        return existing;
    }
    let mask_a = char_to_mask(existing);
    let mask_b = char_to_mask(new);
    // If both are box-drawing, merge their masks.
    if mask_a != 0 && mask_b != 0 {
        return mask_to_char(mask_a | mask_b);
    }
    // Otherwise the new character overwrites.
    new
}

/// Map a box-drawing character to its direction bitmask.
/// Returns 0 for non-box characters.
const fn char_to_mask(ch: char) -> u8 {
    match ch {
        '│' => UP | DOWN,
        '─' => LEFT | RIGHT,
        '┌' => DOWN | RIGHT,
        '┐' => DOWN | LEFT,
        '└' => UP | RIGHT,
        '┘' => UP | LEFT,
        '┬' => DOWN | LEFT | RIGHT,
        '┴' => UP | LEFT | RIGHT,
        '├' => UP | DOWN | RIGHT,
        '┤' => UP | DOWN | LEFT,
        '┼' => UP | DOWN | LEFT | RIGHT,
        '↓' => DOWN,
        _ => 0,
    }
}

/// Map a direction bitmask back to a box-drawing character.
const fn mask_to_char(mask: u8) -> char {
    match mask {
        m if m == UP | DOWN => '│',
        m if m == LEFT | RIGHT => '─',
        m if m == DOWN | RIGHT => '┌',
        m if m == DOWN | LEFT => '┐',
        m if m == UP | RIGHT => '└',
        m if m == UP | LEFT => '┘',
        m if m == DOWN | LEFT | RIGHT => '┬',
        m if m == UP | LEFT | RIGHT => '┴',
        m if m == UP | DOWN | RIGHT => '├',
        m if m == UP | DOWN | LEFT => '┤',
        m if m == UP | DOWN | LEFT | RIGHT => '┼',
        m if m == DOWN => '↓',
        m if m == UP => '↑',
        m if m == LEFT => '─',
        m if m == RIGHT => '─',
        _ => '┼', // fallback for any odd combination
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    use super::*;

    #[test]
    fn merge_vertical_and_horizontal() {
        assert_eq!(merge_box_chars('│', '─'), '┼');
    }

    #[test]
    fn merge_vertical_and_corner() {
        assert_eq!(merge_box_chars('│', '└'), '├');
        assert_eq!(merge_box_chars('│', '┘'), '┤');
    }

    #[test]
    fn merge_horizontal_and_vertical() {
        assert_eq!(merge_box_chars('─', '│'), '┼');
    }

    #[test]
    fn merge_space_yields_other() {
        assert_eq!(merge_box_chars(' ', '│'), '│');
        assert_eq!(merge_box_chars('─', ' '), '─');
    }

    #[test]
    fn merge_same_char() {
        assert_eq!(merge_box_chars('│', '│'), '│');
        assert_eq!(merge_box_chars('─', '─'), '─');
    }

    #[test]
    fn vline_draws_vertical() {
        let mut c = Canvas::new(5, 5);
        c.vline(2, 1, 3);
        assert_eq!(c.cells[1][2], '│');
        assert_eq!(c.cells[3][2], '│');
        assert_eq!(c.cells[0][2], ' ');
    }

    #[test]
    fn to_string_trims_trailing() {
        let mut c = Canvas::new(10, 3);
        c.put_str(0, 0, "hello");
        let s = c.to_string_trimmed();
        assert_eq!(s, "hello");
    }
}
