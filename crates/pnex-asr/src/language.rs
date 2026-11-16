//! Language guard: a model that picks its own language (Parakeet TDT v3)
//! may answer a French segment in English. Measured on a French radio
//! debate: most 30 s windows came out as a loose English translation. The
//! text of every part is checked against the profile language, and a part
//! in another one is transcribed again by a fallback model that is forced
//! to it (Whisper, Canary).

use whatlang::Lang;

use crate::{AsrError, Transcriber, Transcript, Word, SAMPLE_RATE};

/// Below this many words the detection is a guess: the text is kept.
const MIN_WORDS: usize = 5;

/// The detector language of an ISO 639-1 code; `None` = not guarded.
pub fn lang_of(code: &str) -> Option<Lang> {
    let iso3 = match code {
        "bg" => "bul",
        "cs" => "ces",
        "da" => "dan",
        "de" => "deu",
        "el" => "ell",
        "en" => "eng",
        "es" => "spa",
        "et" => "est",
        "fi" => "fin",
        "fr" => "fra",
        "hr" => "hrv",
        "hu" => "hun",
        "it" => "ita",
        "lt" => "lit",
        "lv" => "lav",
        "nl" => "nld",
        "pl" => "pol",
        "pt" => "por",
        "ro" => "ron",
        "ru" => "rus",
        "sk" => "slk",
        "sl" => "slv",
        "sv" => "swe",
        "uk" => "ukr",
        _ => return None,
    };
    Lang::from_code(iso3)
}

/// Whether `text` reads as `lang`; too short to tell = yes.
pub fn is_in(text: &str, lang: Lang) -> bool {
    if text.split_whitespace().count() < MIN_WORDS {
        return true;
    }
    whatlang::detect(text).is_none_or(|info| info.lang() == lang)
}

/// Word times spread over the part by length, for a fallback model that
/// gives none, so speaker turns can still label the words.
// shortcut: proportional to characters, not aligned on the audio; use the
// model's token times once the sherpa runtime returns them for Whisper and Canary.
pub fn spread_words(text: &str, samples: usize) -> Vec<Word> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let total: usize = words.iter().map(|w| w.chars().count() + 1).sum();
    let dur = (samples as u64 * 1000 / u64::from(SAMPLE_RATE)) as u32;
    let mut at = 0usize;
    words
        .into_iter()
        .map(|w| {
            let start = (at as u64 * u64::from(dur) / total.max(1) as u64) as u32;
            at += w.chars().count() + 1;
            let end = (at as u64 * u64::from(dur) / total.max(1) as u64) as u32;
            Word {
                w: w.to_string(),
                start_ms: start,
                end_ms: end,
                p: None,
            }
        })
        .collect()
}

/// `primary`, checked against `lang`; a part in another language is done
/// again by `fallback` (forced to `lang`).
pub struct LanguageGuard {
    pub primary: Box<dyn Transcriber>,
    pub fallback: Box<dyn Transcriber>,
    pub lang: Lang,
}

impl Transcriber for LanguageGuard {
    fn name(&self) -> &str {
        self.primary.name()
    }

    fn transcribe(&mut self, samples: &[f32]) -> Result<Transcript, AsrError> {
        let primary = self.primary.transcribe(samples)?;
        if is_in(&primary.text, self.lang) {
            return Ok(primary);
        }
        // Whisper reads 30 s at most and drops the rest: parts just below
        // it. Equal halves were tried and rejected: Whisper answered "..."
        // to a 15.5 s cut it reads well at 10, 20 or 29 s.
        let mut out = Transcript::default();
        for (i, part) in samples.chunks(FALLBACK_MAX_SAMPLES).enumerate() {
            // shortcut: a tail under 2 s is skipped (the next segment's
            // overlap covers it); transcribe it if segments run without overlap.
            if i > 0 && part.len() < TAIL_MIN_SAMPLES {
                break;
            }
            let mut t = self.fallback.transcribe(part)?;
            if !t.text.chars().any(char::is_alphabetic) {
                // No text at all (Whisper's "..."): the main model's words
                // are better than none.
                return Ok(primary);
            }
            if t.words.is_empty() {
                t.words = spread_words(&t.text, part.len());
            }
            let offset =
                (i as u64 * FALLBACK_MAX_SAMPLES as u64 * 1000 / u64::from(SAMPLE_RATE)) as u32;
            if !out.text.is_empty() {
                out.text.push(' ');
            }
            out.text.push_str(t.text.trim());
            out.words.extend(t.words.into_iter().map(|w| Word {
                start_ms: w.start_ms + offset,
                end_ms: w.end_ms + offset,
                ..w
            }));
        }
        Ok(out)
    }
}

/// Longest audio given to the fallback at once (Whisper's 30 s window).
const FALLBACK_MAX_SAMPLES: usize = 29 * SAMPLE_RATE as usize + SAMPLE_RATE as usize / 2;
/// A shorter last part is not worth a fallback call.
const TAIL_MIN_SAMPLES: usize = 2 * SAMPLE_RATE as usize;

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(&'static str);

    impl Transcriber for Fixed {
        fn name(&self) -> &str {
            self.0
        }
        fn transcribe(&mut self, _: &[f32]) -> Result<Transcript, AsrError> {
            Ok(Transcript {
                text: self.0.into(),
                words: Vec::new(),
            })
        }
    }

    const EN: &str = "Jean Moulin. I'm why, although the history of Jean Moulin is a little bit rich, complex than that.";
    const FR: &str = "Il y a deux figures tutélaires au-dessus du film qui sont dans mon panthéon.";

    #[test]
    fn detects_the_wrong_language() {
        let fr = lang_of("fr").unwrap();
        assert!(!is_in(EN, fr));
        assert!(is_in(FR, fr));
        assert!(is_in("ok yes", fr), "too short to tell");
        assert!(lang_of("auto").is_none());
    }

    #[test]
    fn guard_falls_back_and_times_the_words() {
        let mut g = LanguageGuard {
            primary: Box::new(Fixed(EN)),
            fallback: Box::new(Fixed(FR)),
            lang: lang_of("fr").unwrap(),
        };
        let t = g.transcribe(&vec![0.0; 10 * SAMPLE_RATE as usize]).unwrap();
        assert_eq!(t.text, FR);
        assert_eq!(t.words.len(), FR.split_whitespace().count());
        assert_eq!(t.words.first().unwrap().start_ms, 0);
        assert_eq!(t.words.last().unwrap().end_ms, 10_000);
        assert!(t.words.windows(2).all(|w| w[0].end_ms == w[1].start_ms));
        // In the right language: the primary text stays.
        g.primary = Box::new(Fixed(FR));
        g.fallback = Box::new(Fixed(EN));
        assert_eq!(g.transcribe(&[0.0; 16_000]).unwrap().text, FR);
    }

    #[test]
    fn long_parts_reach_the_fallback_in_pieces() {
        let mut g = LanguageGuard {
            primary: Box::new(Fixed(EN)),
            fallback: Box::new(Fixed(FR)),
            lang: lang_of("fr").unwrap(),
        };
        let n = FR.split_whitespace().count();
        // 31 s: one 29.5 s part, the 1.5 s tail is left to the overlap.
        let t = g.transcribe(&vec![0.0; 31 * SAMPLE_RATE as usize]).unwrap();
        assert_eq!(t.words.len(), n);
        assert_eq!(t.words.last().unwrap().end_ms, 29_500);
        // 62 s: two full parts and a 3 s tail.
        let t = g.transcribe(&vec![0.0; 62 * SAMPLE_RATE as usize]).unwrap();
        assert_eq!(t.words.len(), 3 * n);
        assert_eq!(t.words[n].start_ms, 29_500);
        assert_eq!(t.words.last().unwrap().end_ms, 62_000);
        // A fallback without text keeps the main model's.
        g.fallback = Box::new(Fixed("... ... ..."));
        assert_eq!(g.transcribe(&[0.0; 16_000 * 5]).unwrap().text, EN);
    }
}
