use std::cell::Cell;

use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::HSTRING;

use crate::pixels;

#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub line: u32,
}

pub fn recognize(bgra: &[u8], width: i32, height: i32) -> Result<Vec<Word>, String> {
    init_winrt();
    let engine = engine()?;
    let max_side = OcrEngine::MaxImageDimension().unwrap_or(4096).min(8192) as i32;
    let (fitted, fw, fh, back) = pixels::fit_max(bgra, width, height, max_side);
    let writer = DataWriter::new().map_err(|err| err.to_string())?;
    writer.WriteBytes(&fitted).map_err(|err| err.to_string())?;
    let buffer = writer.DetachBuffer().map_err(|err| err.to_string())?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, fw, fh)
        .map_err(|err| err.to_string())?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|err| err.to_string())?
        .get()
        .map_err(|err| err.to_string())?;
    let lines = result.Lines().map_err(|err| err.to_string())?;
    let count = lines.Size().map_err(|err| err.to_string())?;
    let mut words = Vec::new();
    for line_i in 0..count {
        let line = lines.GetAt(line_i).map_err(|err| err.to_string())?;
        let line_words = line.Words().map_err(|err| err.to_string())?;
        let n = line_words.Size().map_err(|err| err.to_string())?;
        for word_i in 0..n {
            let word = line_words.GetAt(word_i).map_err(|err| err.to_string())?;
            let text = word.Text().map_err(|err| err.to_string())?.to_string();
            if text.trim().is_empty() {
                continue;
            }
            let rect = word.BoundingRect().map_err(|err| err.to_string())?;
            words.push(Word {
                text,
                x: rect.X * back,
                y: rect.Y * back,
                w: rect.Width * back,
                h: rect.Height * back,
                line: line_i,
            });
            if words.len() >= 4000 {
                return Ok(words);
            }
        }
    }
    Ok(words)
}

pub fn join_words(words: &[Word], a: usize, b: usize) -> String {
    if words.is_empty() || a >= words.len() || b >= words.len() {
        return String::new();
    }
    let (start, end) = (a.min(b), a.max(b));
    let mut out = String::new();
    let mut prev: Option<u32> = None;
    for word in &words[start..=end] {
        if let Some(line) = prev {
            if word.line != line {
                out.push('\n');
            } else {
                out.push(' ');
            }
        }
        out.push_str(&word.text);
        prev = Some(word.line);
    }
    out
}

pub fn hit_word(words: &[Word], x: f32, y: f32) -> Option<usize> {
    words.iter().rposition(|word| {
        x >= word.x && y >= word.y && x <= word.x + word.w && y <= word.y + word.h
    })
}

fn engine() -> Result<OcrEngine, String> {
    if let Ok(engine) = OcrEngine::TryCreateFromUserProfileLanguages()
        && engine.RecognizerLanguage().is_ok()
    {
        return Ok(engine);
    }
    if let Ok(langs) = OcrEngine::AvailableRecognizerLanguages()
        && let Ok(count) = langs.Size()
    {
        for i in 0..count {
            if let Ok(lang) = langs.GetAt(i)
                && let Ok(engine) = OcrEngine::TryCreateFromLanguage(&lang)
                && engine.RecognizerLanguage().is_ok()
            {
                return Ok(engine);
            }
        }
    }
    for tag in ["fr-FR", "en-US"] {
        if let Ok(lang) = Language::CreateLanguage(&HSTRING::from(tag))
            && let Ok(engine) = OcrEngine::TryCreateFromLanguage(&lang)
            && engine.RecognizerLanguage().is_ok()
        {
            return Ok(engine);
        }
    }
    Err("OCR Windows indisponible. Ajoute le français dans les langues.".into())
}

fn init_winrt() {
    thread_local! {
        static READY: Cell<bool> = const { Cell::new(false) };
    }
    READY.with(|ready| {
        if ready.get() {
            return;
        }
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        ready.set(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, line: u32) -> Word {
        Word {
            text: text.into(),
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            line,
        }
    }

    #[test]
    fn join_breaks_on_a_new_line() {
        let words = vec![word("Bonjour", 0), word("toi", 0), word("Suite", 1)];
        assert_eq!(join_words(&words, 0, 2), "Bonjour toi\nSuite");
        assert_eq!(join_words(&words, 2, 0), "Bonjour toi\nSuite");
        assert_eq!(join_words(&words, 1, 1), "toi");
    }

    #[test]
    fn hit_picks_the_later_word() {
        let mut words = vec![word("A", 0), word("B", 0)];
        words[0].w = 20.0;
        words[1].x = 5.0;
        words[1].w = 10.0;
        assert_eq!(hit_word(&words, 6.0, 1.0), Some(1));
    }

    #[test]
    #[ignore]
    fn live_ocr_reads_the_desktop() {
        let shot = crate::capture::capture().expect("capture");
        let rect = crate::pixels::PxRect {
            x: 0,
            y: 0,
            w: shot.width.min(1600),
            h: shot.height.min(900),
        };
        let crop = crate::pixels::crop(&shot.bgra, shot.width, rect).expect("crop");
        let words = recognize(&crop, rect.w, rect.h).expect("ocr");
        eprintln!("ocr {} words", words.len());
        for word in words.iter().take(15) {
            eprintln!("  {}", word.text);
        }
        assert!(!words.is_empty(), "aucun mot lu");
    }
}
