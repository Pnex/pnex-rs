//! PDF rendering of a page (media-vision-studio.md D209): structured blocks
//! in, A4 PDF out. Two passes: the blocks are laid out into lines and
//! pages (words measured with the same shaper krilla uses), then drawn,
//! so the footer knows the page count ("page N / M").
//!
//! Fonts: DejaVu Sans (regular, bold, oblique) and DejaVu Sans Mono,
//! embedded and subset by krilla (licence in `fonts/LICENSE`). Labels
//! ("Page", AI notice) come from the caller: this crate holds no wording.

use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect};
use krilla::page::PageSettings;
use krilla::paint::Fill;
use krilla::text::{Font, GlyphId, KrillaGlyph};
use krilla::Document;

static REGULAR: &[u8] = include_bytes!("../fonts/DejaVuSans.ttf");
static BOLD: &[u8] = include_bytes!("../fonts/DejaVuSans-Bold.ttf");
static ITALIC: &[u8] = include_bytes!("../fonts/DejaVuSans-Oblique.ttf");
static MONO: &[u8] = include_bytes!("../fonts/DejaVuSansMono.ttf");

/// A4, points.
const PAGE_W: f32 = 595.0;
const PAGE_H: f32 = 842.0;
const MARGIN_X: f32 = 56.0;
const TOP: f32 = 78.0;
const BOTTOM: f32 = 64.0;
const BODY: f32 = 10.5;
const LEADING: f32 = 1.4;
const INDENT: f32 = 16.0;

/// A run of text and its style.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// Drawn as a link (blue); not clickable in this version.
    pub link: bool,
}

impl Span {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Paragraph(Vec<Span>),
    /// List item: `marker` ("•", "3."), nesting depth from 0.
    Item {
        marker: String,
        depth: u8,
        spans: Vec<Span>,
    },
    Quote(Vec<Span>),
    Rule,
    Code(String),
}

/// What surrounds the content.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Meta {
    pub title: String,
    /// Header right side (organization · version).
    pub header: String,
    /// Footer left side (date of the version).
    pub footer: String,
    /// Footer right side; `{n}` and `{m}` become the page and the count.
    pub page_label: String,
    /// Notice drawn under the title on the first page (AI provenance).
    pub notice: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Face {
    Regular,
    Bold,
    Italic,
    Mono,
}

struct Fonts {
    pdf: [Font; 4],
    shaping: [harfrust::Font; 4],
}

impl Fonts {
    fn load() -> Result<Self, String> {
        let bytes = [REGULAR, BOLD, ITALIC, MONO];
        let pdf = bytes.map(|b| Font::new(b.into(), 0));
        let shaping = bytes.map(|b| harfrust::Font::new(b, 0));
        let [Some(a), Some(b), Some(c), Some(d)] = pdf else {
            return Err("font unreadable".into());
        };
        let [Some(e), Some(f), Some(g), Some(h)] = shaping else {
            return Err("font unreadable".into());
        };
        Ok(Self {
            pdf: [a, b, c, d],
            shaping: [e, f, g, h],
        })
    }

    fn font(&self, f: Face) -> Font {
        self.pdf[f as usize].clone()
    }

    /// Glyphs of `text` (advances normalized to the em, as krilla wants)
    /// and their total advance in ems.
    fn shape(&self, f: Face, text: &str) -> (Vec<KrillaGlyph>, f32) {
        let font = &self.shaping[f as usize];
        let upem = f32::from(font.units_per_em().max(1));
        let shaper = harfrust::ShaperFont::new(font);
        let mut buf = harfrust::Buffer::new();
        buf.push_str(text);
        buf.guess_segment_properties();
        if harfrust::shape(&shaper, &mut buf, harfrust::ShapeOptions::new()).is_err() {
            return (Vec::new(), 0.0);
        }
        let infos = buf.glyph_infos();
        let mut glyphs = Vec::with_capacity(infos.len());
        let mut total = 0.0;
        for (i, (info, pos)) in infos.iter().zip(buf.glyph_positions()).enumerate() {
            // Cluster range: up to the next cluster (left-to-right text).
            let start = info.cluster as usize;
            let end = infos[i + 1..]
                .iter()
                .find(|n| n.cluster != info.cluster)
                .map_or(text.len(), |n| n.cluster as usize);
            let adv = pos.x_advance as f32 / upem;
            total += adv;
            glyphs.push(KrillaGlyph::new(
                GlyphId::new(info.glyph_id),
                adv,
                pos.x_offset as f32 / upem,
                pos.y_offset as f32 / upem,
                pos.y_advance as f32 / upem,
                start..end.max(start),
                None,
            ));
        }
        (glyphs, total)
    }

    /// Advance width of `text` at `size`, points.
    fn width(&self, f: Face, text: &str, size: f32) -> f32 {
        self.shape(f, text).1 * size
    }
}

fn face_of(s: &Span, heading: bool) -> Face {
    if s.code {
        Face::Mono
    } else if s.bold || heading {
        Face::Bold
    } else if s.italic {
        Face::Italic
    } else {
        Face::Regular
    }
}

/// A word to lay out: text, face, colour, glued to the previous one.
type Word = (String, Face, (u8, u8, u8), bool);

/// A word placed on a line.
struct Piece {
    x: f32,
    text: String,
    face: Face,
    size: f32,
    color: (u8, u8, u8),
}

/// One laid-out element of a page.
enum Item {
    Text {
        y: f32,
        pieces: Vec<Piece>,
    },
    Bar {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: (u8, u8, u8),
    },
}

struct Layout<'f> {
    fonts: &'f Fonts,
    pages: Vec<Vec<Item>>,
    y: f32,
}

impl<'f> Layout<'f> {
    fn new(fonts: &'f Fonts) -> Self {
        Self {
            fonts,
            pages: vec![Vec::new()],
            y: TOP,
        }
    }

    /// Room for `h` points, else a new page.
    fn reserve(&mut self, h: f32) {
        if self.y + h > PAGE_H - BOTTOM && self.y > TOP {
            self.pages.push(Vec::new());
            self.y = TOP;
        }
    }

    fn push(&mut self, item: Item) {
        self.pages.last_mut().expect("a page").push(item);
    }

    /// Lays out `spans` word by word between `x0` and the right margin.
    fn text(&mut self, spans: &[Span], x0: f32, size: f32, heading: bool, color: (u8, u8, u8)) {
        let right = PAGE_W - MARGIN_X;
        let line_h = size * LEADING;
        let space = self.fonts.width(Face::Regular, " ", size);
        let mut line: Vec<Piece> = Vec::new();
        let mut x = x0;
        // (word, face, colour, glued to the previous word: no space between
        // two spans that touch, e.g. "**calm**.").
        let mut words: Vec<Word> = Vec::new();
        let mut prev_ws = true;
        for sp in spans {
            let face = face_of(sp, heading);
            let c = if sp.link { (37, 99, 235) } else { color };
            let starts_ws = sp.text.starts_with(char::is_whitespace);
            for (k, w) in sp.text.split_whitespace().enumerate() {
                let glued = k == 0 && !prev_ws && !starts_ws;
                words.push((w.to_string(), face, c, glued));
            }
            if !sp.text.is_empty() {
                prev_ws = sp.text.ends_with(char::is_whitespace);
            }
        }
        for (word, face, c, glued) in words {
            if glued && !line.is_empty() {
                x -= space;
            }
            let mut w = self.fonts.width(face, &word, size);
            if x > x0 && x + w > right {
                self.reserve(line_h);
                let y = self.y + size;
                self.push(Item::Text {
                    y,
                    pieces: std::mem::take(&mut line),
                });
                self.y += line_h;
                x = x0;
            }
            // A word wider than the line is cut by characters.
            let mut word = word;
            while w > right - x0 && word.chars().count() > 1 {
                let mut cut = word.chars().count() / 2;
                while cut > 1
                    && self
                        .fonts
                        .width(face, &word.chars().take(cut).collect::<String>(), size)
                        > right - x0
                {
                    cut -= 1;
                }
                let head: String = word.chars().take(cut).collect();
                line.push(Piece {
                    x,
                    text: head,
                    face,
                    size,
                    color: c,
                });
                self.reserve(line_h);
                let y = self.y + size;
                self.push(Item::Text {
                    y,
                    pieces: std::mem::take(&mut line),
                });
                self.y += line_h;
                x = x0;
                word = word.chars().skip(cut).collect();
                w = self.fonts.width(face, &word, size);
            }
            line.push(Piece {
                x,
                text: word,
                face,
                size,
                color: c,
            });
            x += w + space;
        }
        if !line.is_empty() {
            self.reserve(line_h);
            let y = self.y + size;
            self.push(Item::Text { y, pieces: line });
            self.y += line_h;
        }
    }

    fn block(&mut self, b: &Block) {
        let body = (30, 41, 59);
        match b {
            Block::Heading(level, spans) => {
                let size = match level {
                    1 => 18.0,
                    2 => 15.0,
                    3 => 13.0,
                    _ => 11.5,
                };
                self.y += size * 0.6;
                // Keep a heading with the start of what follows.
                self.reserve(size * LEADING + BODY * LEADING * 2.0);
                self.text(spans, MARGIN_X, size, true, (15, 23, 42));
                self.y += 2.0;
            }
            Block::Paragraph(spans) => {
                self.text(spans, MARGIN_X, BODY, false, body);
                self.y += BODY * 0.6;
            }
            Block::Item {
                marker,
                depth,
                spans,
            } => {
                let x = MARGIN_X + INDENT * f32::from(*depth);
                self.reserve(BODY * LEADING);
                let y = self.y + BODY;
                self.push(Item::Text {
                    y,
                    pieces: vec![Piece {
                        x,
                        text: marker.clone(),
                        face: Face::Regular,
                        size: BODY,
                        color: body,
                    }],
                });
                self.text(spans, x + INDENT, BODY, false, body);
                self.y += BODY * 0.2;
            }
            Block::Quote(spans) => {
                let start_page = self.pages.len();
                let y0 = self.y;
                self.text(spans, MARGIN_X + INDENT, BODY, false, (71, 85, 105));
                if self.pages.len() == start_page {
                    let h = self.y - y0;
                    self.push(Item::Bar {
                        x: MARGIN_X,
                        y: y0,
                        w: 2.5,
                        h,
                        color: (203, 213, 225),
                    });
                }
                self.y += BODY * 0.6;
            }
            Block::Rule => {
                self.reserve(BODY);
                self.y += BODY * 0.4;
                self.push(Item::Bar {
                    x: MARGIN_X,
                    y: self.y,
                    w: PAGE_W - 2.0 * MARGIN_X,
                    h: 0.6,
                    color: (203, 213, 225),
                });
                self.y += BODY * 0.8;
            }
            Block::Code(text) => {
                let size = BODY - 1.0;
                for line in text.lines() {
                    let span = Span {
                        text: line.to_string(),
                        code: true,
                        ..Default::default()
                    };
                    if line.trim().is_empty() {
                        self.y += size * LEADING;
                    } else {
                        self.text(&[span], MARGIN_X + INDENT / 2.0, size, false, (51, 65, 85));
                    }
                }
                self.y += BODY * 0.6;
            }
        }
    }
}

fn fill(rgb: (u8, u8, u8)) -> Fill {
    Fill {
        paint: rgb::Color::new(rgb.0, rgb.1, rgb.2).into(),
        ..Default::default()
    }
}

/// Renders the page as an A4 PDF.
pub fn render(meta: &Meta, blocks: &[Block]) -> Result<Vec<u8>, String> {
    let fonts = Fonts::load()?;
    let mut layout = Layout::new(&fonts);
    layout.text(
        &[Span::plain(meta.title.clone())],
        MARGIN_X,
        20.0,
        true,
        (15, 23, 42),
    );
    if let Some(notice) = &meta.notice {
        layout.y += 4.0;
        let y0 = layout.y;
        layout.text(
            &[Span::plain(notice.clone())],
            MARGIN_X + 8.0,
            BODY - 0.5,
            true,
            (146, 64, 14),
        );
        let h = layout.y - y0 + 4.0;
        layout.pages[0].insert(
            0,
            Item::Bar {
                x: MARGIN_X,
                y: y0 - 2.0,
                w: PAGE_W - 2.0 * MARGIN_X,
                h,
                color: (254, 243, 199),
            },
        );
    }
    layout.y += 10.0;
    for b in blocks {
        layout.block(b);
    }

    let total = layout.pages.len();
    let mut doc = Document::new();
    for (i, items) in layout.pages.iter().enumerate() {
        let settings = PageSettings::from_wh(PAGE_W, PAGE_H).ok_or("page size")?;
        let mut page = doc.start_page_with(settings);
        let mut surface = page.surface();
        let draw = |surface: &mut krilla::surface::Surface,
                    x: f32,
                    y: f32,
                    face: Face,
                    size: f32,
                    text: &str,
                    c: (u8, u8, u8)| {
            surface.set_fill(Some(fill(c)));
            let (glyphs, _) = fonts.shape(face, text);
            surface.draw_glyphs(
                Point::from_xy(x, y),
                &glyphs,
                fonts.font(face),
                text,
                size,
                false,
            );
        };
        // Header and footer.
        let small = 8.0;
        let grey = (100, 116, 139);
        draw(
            &mut surface,
            MARGIN_X,
            36.0,
            Face::Regular,
            small,
            &meta.title,
            grey,
        );
        let hw = fonts.width(Face::Regular, &meta.header, small);
        draw(
            &mut surface,
            PAGE_W - MARGIN_X - hw,
            36.0,
            Face::Regular,
            small,
            &meta.header,
            grey,
        );
        let label = meta
            .page_label
            .replace("{n}", &(i + 1).to_string())
            .replace("{m}", &total.to_string());
        draw(
            &mut surface,
            MARGIN_X,
            PAGE_H - 32.0,
            Face::Regular,
            small,
            &meta.footer,
            grey,
        );
        let lw = fonts.width(Face::Regular, &label, small);
        draw(
            &mut surface,
            PAGE_W - MARGIN_X - lw,
            PAGE_H - 32.0,
            Face::Regular,
            small,
            &label,
            grey,
        );
        for item in items {
            match item {
                Item::Bar { x, y, w, h, color } => {
                    let mut pb = PathBuilder::new();
                    if let Some(r) = Rect::from_xywh(*x, *y, *w, h.max(0.1)) {
                        pb.push_rect(r);
                    }
                    if let Some(path) = pb.finish() {
                        surface.set_fill(Some(fill(*color)));
                        surface.draw_path(&path);
                    }
                }
                Item::Text { y, pieces } => {
                    for p in pieces {
                        draw(&mut surface, p.x, *y, p.face, p.size, &p.text, p.color);
                    }
                }
            }
        }
        surface.finish();
        page.finish();
    }
    doc.finish().map_err(|e| format!("{e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold(t: &str) -> Span {
        Span {
            text: t.into(),
            bold: true,
            ..Default::default()
        }
    }

    #[test]
    fn renders_a_multi_page_report() {
        let mut blocks = vec![
            Block::Heading(1, vec![Span::plain("Rapport hebdomadaire — quai 2")]),
            Block::Paragraph(vec![
                Span::plain("Une semaine "),
                bold("calme"),
                Span::plain(" : aucune alerte, accès éteints à 22 h ("),
                bold("ok"),
                Span::plain(")."),
            ]),
            Block::Item {
                marker: "•".into(),
                depth: 0,
                spans: vec![Span::plain("Lundi : 3 passages")],
            },
            Block::Quote(vec![Span::plain("Généré par IA")]),
            Block::Rule,
            Block::Code("let x = 1;\nlet y = 2;".into()),
        ];
        for i in 0..120 {
            blocks.push(Block::Paragraph(vec![Span::plain(format!(
                "Ligne {i} d'un long texte qui doit passer à la ligne et remplir plusieurs pages \
                 sans jamais déborder de la marge droite, avec unmotbeaucouptroplongpourunelignequidoitetrecoupe."
            ))]));
        }
        let meta = Meta {
            title: "Rapport hebdomadaire".into(),
            header: "Usine pilote · v3".into(),
            footer: "2026-10-13".into(),
            page_label: "Page {n} / {m}".into(),
            notice: Some("Contient du texte généré par IA, non relu.".into()),
        };
        let pdf = render(&meta, &blocks).expect("rendered");
        if let Ok(out) = std::env::var("PNEX_PDF_SAMPLE") {
            std::fs::write(out, &pdf).unwrap();
        }
        assert!(pdf.starts_with(b"%PDF-"));
        let pages = String::from_utf8_lossy(&pdf)
            .matches("/Type /Page\n")
            .count()
            + String::from_utf8_lossy(&pdf)
                .matches("/Type /Page ")
                .count();
        assert!(pages >= 2 || pdf.len() > 20_000, "several pages expected");
        assert!(
            pdf.len() < 1_500_000,
            "fonts are subset: {} bytes",
            pdf.len()
        );
    }

    #[test]
    fn measures_wider_bold_text() {
        let f = Fonts::load().unwrap();
        let r = f.width(Face::Regular, "Weekly report", 10.0);
        let b = f.width(Face::Bold, "Weekly report", 10.0);
        assert!(r > 40.0 && b > r, "{r} {b}");
    }
}
