//! XML parsed safely: every reader of XML (GPX, KML, GML, GeoRSS, LandXML, OGC capabilities, S3
//! listings) goes through [`parse`], which refuses a document nested deeper than [`MAX_DEPTH`]
//! before parsing it. Nesting thousands of elements deep exhausts the stack while the document is
//! parsed or walked; a hostile file must be an error, never a crash.

use roxmltree::{Document, ParsingOptions};

/// How deep elements may nest. Real files stay far below it (KML folders, GML geometry and WMS
/// layer trees are a few dozen deep at most).
pub const MAX_DEPTH: usize = 256;

/// Parses `text` (a leading byte-order mark dropped) with `opts`, once [`check_depth`] passes.
pub fn parse<'a>(text: &'a str, opts: ParsingOptions<'a>) -> Result<Document<'a>, String> {
    let text = text.trim_start_matches('\u{feff}');
    check_depth(text, opts.allow_dtd)?;
    Document::parse_with_options(text, opts).map_err(|e| e.to_string())
}

/// Refuses XML nested deeper than [`MAX_DEPTH`], by a scan that never recurses. It counts what
/// opens and closes an element, skipping comments, CDATA, processing instructions and quoted
/// attribute values; where it can't tell, it counts too deep rather than too shallow. With a DTD
/// allowed, an entity whose value holds markup is refused too: expanded, it would nest in ways
/// the text doesn't show.
pub fn check_depth(text: &str, allow_dtd: bool) -> Result<(), String> {
    let b = text.as_bytes();
    let (mut i, mut depth) = (0usize, 0usize);
    let find = |from: usize, pat: &[u8]| -> Option<usize> { b.get(from..)?.windows(pat.len()).position(|w| w == pat).map(|p| from + p) };
    while i < b.len() {
        if b.get(i) != Some(&b'<') {
            i += 1;
            continue;
        }
        let rest = b.get(i..).unwrap_or(&[]);
        if rest.starts_with(b"<!--") {
            match find(i + 4, b"-->") {
                Some(e) => i = e + 3,
                None => return Ok(()),
            }
        } else if rest.starts_with(b"<![CDATA[") {
            match find(i + 9, b"]]>") {
                Some(e) => i = e + 3,
                None => return Ok(()),
            }
        } else if rest.starts_with(b"<?") {
            match find(i + 2, b"?>") {
                Some(e) => i = e + 2,
                None => return Ok(()),
            }
        } else if rest.starts_with(b"<!") {
            // A DOCTYPE (with any internal subset): to its closing `>`, `[ … ]` and quotes skipped.
            // Comments and processing instructions in the subset are skipped whole (an apostrophe in
            // one isn't a quote). A declaration that never ends is refused: the rest of the
            // document would go uncounted.
            let mut j = i + 2;
            let (mut bracket, mut quote) = (0usize, None::<u8>);
            let mut closed = false;
            while let Some(&c) = b.get(j) {
                if quote.is_none() && b.get(j..).is_some_and(|r| r.starts_with(b"<!--")) {
                    j = find(j + 4, b"-->").map(|e| e + 3).ok_or("the document's DOCTYPE has an unclosed comment")?;
                    continue;
                }
                if quote.is_none() && b.get(j..).is_some_and(|r| r.starts_with(b"<?")) {
                    j = find(j + 2, b"?>").map(|e| e + 2).ok_or("the document's DOCTYPE has an unclosed instruction")?;
                    continue;
                }
                match (quote, c) {
                    (Some(q), c) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(c),
                    (None, b'[') => bracket += 1,
                    (None, b']') => bracket = bracket.saturating_sub(1),
                    (None, b'>') if bracket == 0 => {
                        closed = true;
                        break;
                    }
                    _ => {}
                }
                j += 1;
            }
            if !closed {
                return Err("the document's DOCTYPE never ends".into());
            }
            let decl = b.get(i..j).unwrap_or(&[]);
            if allow_dtd && entity_with_markup(decl) {
                return Err("the document declares entities holding markup, which aren't read".into());
            }
            i = j + 1;
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = find(i + 2, b">").map(|e| e + 1).unwrap_or(b.len());
        } else {
            // An element's start tag: to its `>` (quoted values skipped); `/>` closes it at once.
            depth += 1;
            if depth > MAX_DEPTH {
                return Err(format!("the document nests elements more than {MAX_DEPTH} deep"));
            }
            let mut j = i + 1;
            let mut quote = None::<u8>;
            while let Some(&c) = b.get(j) {
                match (quote, c) {
                    (Some(q), c) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(c),
                    (None, b'>') => break,
                    _ => {}
                }
                j += 1;
            }
            if j > 0 && b.get(j - 1) == Some(&b'/') {
                depth = depth.saturating_sub(1);
            }
            i = j + 1;
        }
    }
    Ok(())
}

/// Whether a DOCTYPE declares an entity whose value holds markup (a `<`).
fn entity_with_markup(decl: &[u8]) -> bool {
    let mut at = 0;
    while let Some(p) = decl.get(at..).and_then(|r| r.windows(8).position(|w| w == b"<!ENTITY")) {
        let start = at + p + 8;
        // The declaration runs to the first `>` outside quotes (a quoted value may hold `>`); a
        // `<` anywhere in it, quoted or not, is markup.
        let mut quote = None::<u8>;
        let mut end = decl.len();
        for (k, &c) in decl.get(start..).unwrap_or(&[]).iter().enumerate() {
            match (quote, c) {
                (_, b'<') => return true,
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, b'"' | b'\'') => quote = Some(c),
                (None, b'>') => {
                    end = start + k;
                    break;
                }
                _ => {}
            }
        }
        at = end;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(n: usize) -> String {
        format!("<?xml version=\"1.0\"?><r>{}{}</r>", "<a>".repeat(n), "</a>".repeat(n))
    }

    #[test]
    fn depth_is_bounded() {
        assert!(check_depth(&nested(200), false).is_ok());
        assert!(check_depth(&nested(10_000), false).is_err());
        // Self-closed, commented, CDATA and quoted `<`s don't nest.
        let flat = format!(
            "<r>{}<!-- {} --><![CDATA[{}]]><a v=\"{}\"/></r>",
            "<a/>".repeat(1000),
            "<a>".repeat(1000),
            "<a>".repeat(1000),
            "<a>".repeat(300)
        );
        assert!(check_depth(&flat, false).is_ok());
        // Unclosed opens count (a truncated file never reads as shallow).
        assert!(check_depth(&"<a>".repeat(300), false).is_err());
        let (deep, shallow) = (nested(10_000), nested(10));
        assert!(parse(&deep, ParsingOptions::default()).is_err());
        assert!(parse(&shallow, ParsingOptions::default()).is_ok());
        // An entity holding markup, with a DTD allowed: refused; plain ones are fine.
        let ent = "<!DOCTYPE r [<!ENTITY a \"<b><b>&a;</b></b>\">]><r>&a;</r>";
        assert!(check_depth(ent, true).is_err());
        // A `>` in the quoted value before its markup doesn't hide the markup.
        for hidden in ["<!DOCTYPE r [<!ENTITY a \"x>&b;<c>\">]><r/>", "<!DOCTYPE r [<!ENTITY a 'x>y<c>'>]><r/>"] {
            assert!(check_depth(hidden, true).is_err(), "{hidden}");
        }
        // A comment or instruction in the subset with an apostrophe doesn't hide the document
        // after it: its nesting is still counted.
        for subset in ["<!-- it's -->", "<?pi it's?>"] {
            let doc = format!("<!DOCTYPE r [ {subset} ]>{}", nested(10_000));
            assert!(check_depth(&doc, true).is_err() && check_depth(&doc, false).is_err(), "{subset}");
            let ok = format!("<!DOCTYPE r [ {subset} ]><r/>");
            assert!(check_depth(&ok, true).is_ok(), "{subset}");
        }
        // A DOCTYPE that never ends is refused rather than skipping the rest.
        assert!(check_depth(&format!("<!DOCTYPE r [ '{}", nested(10)), true).is_err());
        let plain =
            "<!DOCTYPE WMT_MS_Capabilities SYSTEM \"http://x/WMS_MS_Capabilities.dtd\" [<!ELEMENT VendorSpecificCapabilities EMPTY>]><r/>";
        assert!(check_depth(plain, true).is_ok());
    }

    /// Every XML reader refuses a deeply nested document instead of overflowing the stack, run on
    /// a small one (debug builds use the most stack per frame).
    #[test]
    fn every_reader_refuses_deep_nesting() {
        let r = std::thread::Builder::new()
            .stack_size(512 << 10)
            .spawn(|| {
                let deep = |root: &str, inner: &str| {
                    format!(
                        "<?xml version=\"1.0\"?><{root}>{}{}</{root}>",
                        format!("<{inner}>").repeat(10_000),
                        format!("</{inner}>").repeat(10_000)
                    )
                    .into_bytes()
                };
                assert!(crate::gpx::read("a.gpx", &deep("gpx", "trk")).is_err());
                assert!(crate::kml::read("a.kml", &deep("kml", "Folder")).is_err());
                assert!(crate::kml::ground_overlays("a.kml", &deep("kml", "Folder")).is_err());
                assert!(crate::gml::read("a.gml", &deep("wfs:FeatureCollection", "gml:featureMember")).is_err());
                assert!(crate::georss::read("a.rss", &deep("rss", "channel")).is_err());
                assert!(crate::landxml::read("a.xml", &deep("LandXML", "Parcels"), None).is_err());
                assert!(crate::ogc::parse("https://x.example/wms", &deep("WMS_Capabilities", "Layer")).is_err());
                true
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(r);
    }

    /// A deep document is refused on a small stack (as a thread of the app's might have), never
    /// overflowing it.
    #[test]
    fn deep_documents_never_overflow_a_small_stack() {
        let text = nested(100_000);
        let r = std::thread::Builder::new()
            .stack_size(256 << 10)
            .spawn(move || parse(&text, ParsingOptions::default()).is_err())
            .unwrap()
            .join()
            .unwrap();
        assert!(r);
    }
}
