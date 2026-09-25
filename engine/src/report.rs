//! Pro edition: minimal single-purpose PDF report writer (PDF 1.4,
//! Helvetica, Letter portrait). No external crates — just a text-stream
//! emitter with a correct xref table. Same approach as the fea-lite
//! report: paginated styled lines plus ruled tables.

/// One line in the report body.
pub enum Row {
    /// Document title (18 pt bold).
    Title(String),
    /// Section heading (13 pt bold).
    Head(String),
    /// Body text (10 pt regular).
    Body(String),
    /// Muted note (9 pt regular, gray).
    Note(String),
    /// Vertical gap (points).
    Gap(f64),
    /// A table: header row + data rows, equal column widths across the
    /// text width, light rules between rows.
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
        /// Relative column widths (must sum to ~1.0; default equal).
        weights: Option<Vec<f64>>,
    },
}

/// Renders the rows into a PDF document (bytes).
pub fn render_pdf(_title: &str, rows: &[Row]) -> Vec<u8> {
    const PAGE_W: f64 = 612.0;
    const PAGE_H: f64 = 792.0;
    const MARGIN: f64 = 56.0;
    const TEXT_W: f64 = PAGE_W - 2.0 * MARGIN;
    const LINE: f64 = 15.0;

    // ---- layout: break rows into pages of positioned lines ----
    enum Op {
        Text { x: f64, y: f64, size: f64, bold: bool, gray: bool, text: String },
        Rule { y: f64 },
    }
    let mut pages: Vec<Vec<Op>> = vec![Vec::new()];
    let mut y = PAGE_H - MARGIN;

    let push_page = |pages: &mut Vec<Vec<Op>>, y: &mut f64| {
        pages.push(Vec::new());
        *y = PAGE_H - MARGIN;
    };

    for row in rows {
        let (size, bold, gray, text) = match row {
            Row::Title(t) => (18.0, true, false, t),
            Row::Head(t) => (13.0, true, false, t),
            Row::Body(t) => (10.0, false, false, t),
            Row::Note(t) => (9.0, false, true, t),
            Row::Gap(h) => {
                y -= h;
                continue;
            }
            Row::Table { columns, rows: data, weights } => {
                let n = columns.len().max(1);
                let widths: Vec<f64> = match weights {
                    Some(w) if w.len() == n => w.iter().map(|&x| x * TEXT_W).collect(),
                    _ => vec![TEXT_W / n as f64; n],
                };
                // Header.
                if y < MARGIN + 3.0 * LINE {
                    push_page(&mut pages, &mut y);
                }
                {
                    let page = pages.last_mut().expect("at least one page");
                    let mut x = MARGIN;
                    for (i, col) in columns.iter().enumerate() {
                        page.push(Op::Text { x, y, size: 10.0, bold: true, gray: false, text: escape(col) });
                        x += widths[i];
                    }
                }
                y -= LINE;
                // Rule under the header:
                pages.last_mut().expect("page").push(Op::Rule { y: y + 4.0 });
                for data_row in data {
                    if y < MARGIN + LINE {
                        push_page(&mut pages, &mut y);
                        // repeat header on continuation pages
                        let page = pages.last_mut().expect("page");
                        let mut x = MARGIN;
                        for (i, col) in columns.iter().enumerate() {
                            page.push(Op::Text { x, y, size: 10.0, bold: true, gray: false, text: escape(col) });
                            x += widths[i];
                        }
                        y -= LINE;
                        pages.last_mut().expect("page").push(Op::Rule { y: y + 4.0 });
                    }
                    let page = pages.last_mut().expect("page");
                    let mut x = MARGIN;
                    for (i, cell) in data_row.iter().enumerate() {
                        page.push(Op::Text { x, y, size: 10.0, bold: false, gray: false, text: escape(cell) });
                        x += widths.get(i).copied().unwrap_or(0.0);
                    }
                    pages.last_mut().expect("page").push(Op::Rule { y: y - 4.0 });
                    y -= LINE;
                }
                pages.last_mut().expect("page").push(Op::Rule { y: y + 4.0 + 4.0 });
                y -= 4.0;
                continue;
            }
        };
        if y < MARGIN + LINE {
            push_page(&mut pages, &mut y);
        }
        pages.last_mut().expect("page").push(Op::Text {
            x: MARGIN,
            y,
            size,
            bold,
            gray,
            text: escape(text),
        });
        y -= size + 6.0;
    }

    // ---- emit PDF objects ----
    let n_pages = pages.len();
    // Object numbering: 1 catalog, 2 pages, then per page [page, contents],
    // then fonts: F1 regular, F2 bold.
    let first_page_obj = 3usize;
    let font_regular_obj = first_page_obj + 2 * n_pages;
    let font_bold_obj = font_regular_obj + 1;

    let mut objects: Vec<(usize, Vec<u8>)> = Vec::new();
    objects.push((1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()));
    let kids: Vec<String> = (0..n_pages)
        .map(|i| format!("{} 0 R", first_page_obj + 2 * i))
        .collect();
    objects.push((
        2,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            n_pages
        )
        .into_bytes(),
    ));

    for (page_index, ops) in pages.iter().enumerate() {
        let page_obj = first_page_obj + 2 * page_index;
        let content_obj = page_obj + 1;
        objects.push((
            page_obj,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] \
                 /Contents {content_obj} 0 R /Resources << /Font \
                 << /F1 {font_regular_obj} 0 R /F2 {font_bold_obj} 0 R >> >> >>"
            )
            .into_bytes(),
        ));

        let mut stream = String::new();
        for op in ops {
            match op {
                Op::Text { x, y, size, bold, gray, text } => {
                    let font = if *bold { "F2" } else { "F1" };
                    let gray_op = if *gray { " 0.45 g" } else { " 0 g" };
                    stream.push_str(&format!(
                        "BT{gray_op} /{font} {size} Tf 1 0 0 1 {x} {y} Tm ({text}) Tj ET\n"
                    ));
                }
                Op::Rule { y } => {
                    stream.push_str(&format!(
                        "0.8 G 0.5 w {MARGIN} {y} m {} {y} l S\n",
                        MARGIN + TEXT_W
                    ));
                }
            }
        }
        objects.push((
            content_obj,
            format!("<< /Length {} >>\nstream\n{}\nendstream", stream.len(), stream).into_bytes(),
        ));
    }

    objects.push((
        font_regular_obj,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
    ));
    objects.push((
        font_bold_obj,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    ));

    // ---- serialize with a correct xref ----
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
    let mut offsets = vec![0usize; objects.len() + 1];
    for (num, body) in &objects {
        offsets[*num] = out.len();
        out.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_at = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for num in 1..=objects.len() {
        out.extend_from_slice(format!("{:010} 00000 n \n", offsets[num]).as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref_at
        )
        .as_bytes(),
    );
    out
}

/// Escapes a string for a PDF literal (parentheses and backslashes), and
/// transliterates the characters WinAnsi can't carry to safe ASCII.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\\' => out.push_str("\\\\"),
            '\u{b7}' | '\u{2022}' => out.push('\u{b7}'), // middle dot exists in WinAnsi
            '\u{d7}' => out.push('x'),                   // multiplication sign -> x
            '\u{3bd}' => out.push_str("nu"),             // Greek nu
            '\u{221e}' => out.push_str("inf"),
            c if (c as u32) < 128 => out.push(c),
            c if (c as u32) < 256 => out.push(c), // WinAnsi range
            _ => out.push('?'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_a_parseable_pdf() {
        let rows = [
            Row::Title("CFD Report".to_string()),
            Row::Note("TPT CFD Studio Pro (test)".to_string()),
            Row::Gap(6.0),
            Row::Head("Problem".to_string()),
            Row::Body("Sphere, 100 mm frontal height, 1.5 m/s air".to_string()),
            Row::Head("Results".to_string()),
            Row::Table {
                columns: ["Quantity".to_string(), "Value".to_string(), "Detail".to_string()]
                    .to_vec(),
                rows: vec![
                    vec!["Drag coefficient".to_string(), "1.42".to_string(), "time-averaged".to_string()],
                    vec!["Lift coefficient".to_string(), "0.01".to_string(), "time-averaged".to_string()],
                ],
                weights: Some(vec![0.4, 0.26, 0.34]),
            },
            Row::Note("Reynolds number above the stable band is shown at the ceiling.".to_string()),
        ];
        let pdf = render_pdf("CFD Report", &rows);
        let text = String::from_utf8_lossy(&pdf);
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(text.trim_end().ends_with("%%EOF"));
        assert!(text.contains("/Count 1"));
        assert!(text.contains("Drag coefficient"));
        // Every object offset in the xref must point at "N 0 obj".
        let xref_at = text.rfind("xref").expect("xref");
        for line in text[xref_at..].lines().skip(2) {
            if line.len() == 20 {
                let offset: usize = line[..10].trim().parse().unwrap();
                if offset > 0 {
                    assert!(text[offset..].starts_with(char::is_numeric as fn(char) -> bool));
                }
            }
        }
    }
}
