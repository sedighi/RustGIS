//! Spreadsheets: Excel (`.xlsx`, `.xlsm`, `.xlsb`, `.xls`) and OpenDocument (`.ods`) workbooks,
//! read in pure Rust (in the browser too). A sheet is read like delimited text: its first
//! non-empty row names the columns, and x/y or WKT columns (found, or given with
//! [`CsvOptions`]) make the geometry; a sheet without them is a table.

use std::io::Cursor;

use calamine::{Data, ExcelDateTime, ExcelDateTimeType, Reader, Sheets, open_workbook_auto_from_rs};

use crate::io::{Columns, CsvOptions, columns_of, read_rows};
use crate::{DataError, Dataset, ReadReport, Result};

/// Spreadsheet file types.
pub const EXTENSIONS: &[&str] = &["xlsx", "xlsm", "xlsb", "xls", "ods"];

/// The most cells read from one sheet (calamine holds a sheet as a dense grid, and its cells are
/// copied as text).
const MAX_CELLS: usize = if cfg!(target_arch = "wasm32") { 4_000_000 } else { 20_000_000 };

/// Whether a file name is a spreadsheet.
pub fn is_sheet_name(name: &str) -> bool {
    let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    EXTENSIONS.contains(&ext.as_str())
}

/// The sheets of a workbook, in order.
pub fn sheet_names(bytes: &[u8]) -> Result<Vec<String>> {
    check_expansion(bytes)?;
    let wb = open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|e| DataError::Format(format!("spreadsheet: {e}")))?;
    Ok(wb.sheet_names())
}

/// Reads one sheet of a workbook (`sheet` by name, without regard to case; the first when `None`),
/// named `<name>` (or `<name> <sheet>` when the workbook has several). The report names the other
/// sheets.
pub fn read(bytes: &[u8], name: &str, sheet: Option<&str>, opts: &CsvOptions) -> Result<(Dataset, ReadReport)> {
    let g = grid(bytes, sheet)?;
    let title = if g.names.len() > 1 { format!("{name} {}", g.pick) } else { name.to_string() };
    let (ds, mut report) = read_rows(&g.headers, &g.rows, 0, &title, opts)?;
    let others: Vec<&str> = g.names.iter().filter(|n| **n != g.pick).map(String::as_str).take(20).collect();
    if !others.is_empty() {
        report.messages.push(format!("the workbook has more sheets ({}): add one with \"layer\"", others.join(", ")));
    }
    Ok((ds, report))
}

/// The columns of one sheet of a workbook (`sheet` as [`read`] takes it), and those its geometry
/// is read from when none are given.
pub fn columns(bytes: &[u8], sheet: Option<&str>) -> Result<Columns> {
    let g = grid(bytes, sheet)?;
    Ok(columns_of(&g.headers, &g.rows))
}

/// A sheet as text: the workbook's sheets, the one picked, its column names and its rows.
struct Grid {
    names: Vec<String>,
    pick: String,
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

fn grid(bytes: &[u8], sheet: Option<&str>) -> Result<Grid> {
    check_expansion(bytes)?;
    let mut wb = open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|e| DataError::Format(format!("spreadsheet: {e}")))?;
    let names = wb.sheet_names();
    let pick = match sheet.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => names
            .iter()
            .find(|n| n.eq_ignore_ascii_case(s))
            .cloned()
            .ok_or_else(|| DataError::Format(format!("no sheet '{s}' (sheets: {})", names.join(", "))))?,
        None => names.first().cloned().ok_or_else(|| DataError::Format("the workbook has no sheets".into()))?,
    };
    check_size(&mut wb, &pick)?;
    let range = wb.worksheet_range(&pick).map_err(|e| DataError::Format(format!("sheet '{pick}': {e}")))?;
    let (h, w) = range.get_size();
    if h.saturating_mul(w) > MAX_CELLS {
        return Err(DataError::Format(format!("sheet '{pick}' is too large ({h} × {w} cells)")));
    }
    let mut rows = range.rows().map(|r| r.iter().map(cell).collect::<Vec<String>>());
    // The first row with anything in it names the columns.
    let headers: Vec<String> = rows.by_ref().find(|r| r.iter().any(|c| !c.trim().is_empty())).unwrap_or_default();
    if headers.is_empty() {
        return Err(DataError::Format(format!("sheet '{pick}' is empty")));
    }
    // Columns without a heading are named by their letter, as the spreadsheet shows them (the
    // range starts at its first column with anything in it).
    let first_col = range.start().map(|(_, c)| c as usize).unwrap_or(0);
    let headers: Vec<String> = headers
        .into_iter()
        .enumerate()
        .map(|(i, h)| if h.trim().is_empty() { column_letter(first_col + i) } else { h.trim().to_string() })
        .collect();
    let rows: Vec<Vec<String>> = rows.filter(|r| r.iter().any(|c| !c.trim().is_empty())).collect();
    Ok(Grid { names, pick, headers, rows })
}

/// A zipped workbook (`.xlsx`, `.ods`) that would inflate past [`crate::zip::MAX_TOTAL`] is
/// refused before it is read: the archive's own reader inflates each part within that budget,
/// checking its size and checksum against what the archive says (calamine trusts the headers).
fn check_expansion(bytes: &[u8]) -> Result<()> {
    if !bytes.starts_with(b"PK\x03\x04") {
        return Ok(());
    }
    crate::zip::read_matching_within(bytes, |n| n.ends_with(".xml") || n.ends_with(".bin"), crate::zip::MAX_TOTAL)
        .map(|_| ())
        .map_err(|e| DataError::Format(format!("spreadsheet: {e}")))
}

/// Refuses a sheet whose cells span more than [`MAX_CELLS`] before calamine lays it out as a dense
/// grid (one cell at `XFD1048576` would ask for 17 billion): an `.xlsx` or `.xlsb` sheet's cells
/// are read one at a time for their extent first. An `.xls` sheet can't be larger than 65,536 ×
/// 256 cells, and an `.ods` one is checked after reading.
fn check_size(wb: &mut Sheets<Cursor<&[u8]>>, sheet: &str) -> Result<()> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    let mut grow = |(r, c): (u32, u32)| -> Result<()> {
        let (r0, r1, c0, c1) = b.map_or((r, r, c, c), |(r0, r1, c0, c1)| (r0.min(r), r1.max(r), c0.min(c), c1.max(c)));
        b = Some((r0, r1, c0, c1));
        let cells = ((r1 - r0) as usize + 1).saturating_mul((c1 - c0) as usize + 1);
        if cells > MAX_CELLS {
            return Err(DataError::Format(format!(
                "sheet '{sheet}' is too large (cells from row {} to {}, column {} to {})",
                r0 + 1,
                r1 + 1,
                column_letter(c0 as usize),
                column_letter(c1 as usize)
            )));
        }
        Ok(())
    };
    let bad = |e: String| DataError::Format(format!("sheet '{sheet}': {e}"));
    match wb {
        Sheets::Xlsx(x) => {
            let mut cells = x.worksheet_cells_reader(sheet).map_err(|e| bad(e.to_string()))?;
            while let Some(c) = cells.next_cell().map_err(|e| bad(e.to_string()))? {
                grow(c.get_position())?;
            }
        }
        Sheets::Xlsb(x) => {
            let mut cells = x.worksheet_cells_reader(sheet).map_err(|e| bad(e.to_string()))?;
            while let Some(c) = cells.next_cell().map_err(|e| bad(e.to_string()))? {
                grow(c.get_position())?;
            }
        }
        Sheets::Xls(_) | Sheets::Ods(_) => {}
    }
    Ok(())
}

/// A cell as text: whole numbers without a decimal point, dates as ISO 8601, times of day as
/// `HH:MM:SS` and durations as `[h]:MM:SS`.
fn cell(d: &Data) -> String {
    match d {
        Data::Empty | Data::Error(_) => String::new(),
        Data::String(s) => s.clone(),
        Data::Bool(b) => b.to_string(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
        Data::Float(f) => f.to_string(),
        Data::DateTime(dt) if dt.is_duration() => duration(dt.as_f64()),
        Data::DateTime(dt) => excel_date(dt.as_f64() + if is_1904(dt) { DAYS_1900_TO_1904 } else { 0.0 }, true),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
    }
}

/// Days from the 1900 date system's day 0 to the 1904 one's (workbooks made on old Macs).
const DAYS_1900_TO_1904: f64 = 1462.0;

/// Whether a date counts its days from 1904: calamine converts it by its workbook's date system,
/// so the same serial converted from 1900 comes out differently.
fn is_1904(dt: &ExcelDateTime) -> bool {
    let v = dt.as_f64();
    v.is_finite()
        && (0.0..3_000_000.0).contains(&v)
        && dt.to_ymd_hms_milli() != ExcelDateTime::new(v, ExcelDateTimeType::DateTime, false).to_ymd_hms_milli()
}

/// A duration in days as `[h]:MM:SS` (hours past 24 kept, as Excel shows `[h]:mm:ss`).
fn duration(days: f64) -> String {
    if !days.is_finite() || days.abs() > 1e9 {
        return days.to_string();
    }
    let secs = (days.abs() * 86_400.0).round() as i64;
    let sign = if days < 0.0 && secs > 0 { "-" } else { "" };
    format!("{sign}{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

/// An Excel serial date (days since 1899-12-30) as `YYYY-MM-DD`, or `YYYY-MM-DDTHH:MM:SS` when it
/// has a time of day; a time alone (a serial below 1) as `HH:MM:SS`.
fn excel_date(serial: f64, with_time: bool) -> String {
    if !serial.is_finite() || !(0.0..3_000_000.0).contains(&serial) {
        return serial.to_string();
    }
    if serial < 1.0 && with_time {
        let secs = ((serial * 86_400.0).round() as i64).min(86_399);
        return format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
    }
    let mut days = serial.floor() as i64;
    let mut secs = ((serial - serial.floor()) * 86_400.0).round() as i64;
    // A time that rounds up to midnight is the next day.
    if secs >= 86_400 {
        days += 1;
        secs -= 86_400;
    }
    // Excel counts a 29 February 1900 that never was: serials before it are a day early.
    if days < 60 {
        days += 1;
    }
    // Days from 1970-01-01, then the civil date (Howard Hinnant's algorithm).
    let z = days - 25_569 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    if with_time && secs > 0 {
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{y:04}-{m:02}-{d:02}")
    }
}

/// A column's letter name: A, B, …, Z, AA, AB…
fn column_letter(mut i: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (i % 26) as u8);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// A minimal `.xlsx`: sheets of rows of cells (numbers, or inline strings), for tests.
#[doc(hidden)]
pub fn xlsx(sheets: &[(&str, &[&[&str]])]) -> Vec<u8> {
    // A cell given as `<c …>` XML goes in as it is (to place it with `r="…"`).
    let cell = |v: &str| {
        if v.starts_with("<c") {
            v.to_string()
        } else if v.parse::<f64>().is_ok() {
            format!("<c><v>{v}</v></c>")
        } else {
            format!("<c t=\"inlineStr\"><is><t>{v}</t></is></c>")
        }
    };
    let mut entries = vec![
        (
            "[Content_Types].xml".to_string(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>{}</Types>"#,
                (1..=sheets.len())
                    .map(|i| format!(r#"<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#))
                    .collect::<String>()
            )
            .into_bytes(),
        ),
        (
            "_rels/.rels".to_string(),
            br#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_vec(),
        ),
        (
            "xl/workbook.xml".to_string(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{}</sheets></workbook>"#,
                sheets.iter().enumerate().map(|(i, (n, _))| format!(r#"<sheet name="{n}" sheetId="{}" r:id="rId{}"/>"#, i + 1, i + 1)).collect::<String>()
            )
            .into_bytes(),
        ),
        (
            "xl/_rels/workbook.xml.rels".to_string(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}</Relationships>"#,
                (1..=sheets.len())
                    .map(|i| format!(r#"<Relationship Id="rId{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{i}.xml"/>"#))
                    .collect::<String>()
            )
            .into_bytes(),
        ),
    ];
    for (i, (_, rows)) in sheets.iter().enumerate() {
        let data: String = rows.iter().map(|r| format!("<row>{}</row>", r.iter().map(|v| cell(v)).collect::<String>())).collect();
        entries.push((
            format!("xl/worksheets/sheet{}.xml", i + 1),
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{data}</sheetData></worksheet>"#).into_bytes(),
        ));
    }
    crate::zip::write(&entries).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_letters() {
        assert_eq!(excel_date(45_000.0, false), "2023-03-15");
        assert_eq!(excel_date(25_569.5, true), "1970-01-01T12:00:00");
        assert_eq!(excel_date(1.0, false), "1900-01-01");
        assert_eq!(excel_date(59.0, false), "1900-02-28");
        assert_eq!(excel_date(61.0, false), "1900-03-01");
        assert_eq!(excel_date(45_000.999_999_9, true), "2023-03-16", "rounds up to the next midnight");
        assert_eq!(excel_date(0.5, true), "12:00:00", "a time of day alone");
        assert_eq!(excel_date(0.0, true), "00:00:00");
        assert_eq!(duration(1.5), "36:00:00");
        assert_eq!(duration(-0.25), "-6:00:00");
        // A 1904-system date is the same day as its 1900-system serial plus 1462.
        let d1904 = ExcelDateTime::new(45_000.0 - DAYS_1900_TO_1904, ExcelDateTimeType::DateTime, true);
        assert!(is_1904(&d1904));
        assert_eq!(cell(&Data::DateTime(d1904)), "2023-03-15");
        let d1900 = ExcelDateTime::new(45_000.0, ExcelDateTimeType::DateTime, false);
        assert!(!is_1904(&d1900));
        assert_eq!(cell(&Data::DateTime(d1900)), "2023-03-15");
        assert_eq!(column_letter(0), "A");
        assert_eq!(column_letter(25), "Z");
        assert_eq!(column_letter(26), "AA");
        assert_eq!(column_letter(701), "ZZ");
        assert!(is_sheet_name("a.XLSX") && is_sheet_name("b.ods") && !is_sheet_name("c.csv"));
    }

    #[test]
    fn reads_sheets() {
        let book = xlsx(&[
            (
                "Cities",
                &[
                    &["name", "lon", "lat", "pop"],
                    &["Paris", "2.35", "48.85", "2100000"],
                    &["", "", "", ""],
                    &["Rome", "12.5", "41.9", "2800000"],
                ],
            ),
            ("Utm", &[&["", "", ""], &["id", "easting", "northing"], &["1", "500000", "4649776"]]),
            ("Notes", &[&["text", ""], &["hello", "x"]]),
        ]);
        assert_eq!(sheet_names(&book).unwrap(), ["Cities", "Utm", "Notes"]);
        let (ds, rep) = read(&book, "book", None, &CsvOptions::default()).unwrap();
        assert_eq!(ds.name, "book Cities");
        assert_eq!(ds.features.len(), 2, "the empty row is passed over");
        assert_eq!(ds.geometry_type, crate::GeometryType::Point);
        assert_eq!(ds.fields.iter().find(|f| f.name == "pop").map(|f| f.ty), Some(crate::FieldType::Integer));
        assert!(rep.messages.iter().any(|m| m.contains("Utm, Notes")));
        // The header row comes after a blank one; the coordinates are UTM 33N.
        let opts = CsvOptions { crs: crate::raster::Crs::from_epsg(32633), ..CsvOptions::default() };
        let (ds, _) = read(&book, "book", Some("utm"), &opts).unwrap();
        let e = ds.features[0].geometry.as_ref().unwrap().extent();
        assert!((e.min_x - 15.0).abs() < 0.01 && (e.min_y - 42.0).abs() < 0.05, "{e:?}");
        let (ds, _) = read(&book, "book", Some("Notes"), &CsvOptions::default()).unwrap();
        assert_eq!(ds.geometry_type, crate::GeometryType::None);
        assert_eq!(ds.fields[1].name, "B", "a column without a heading is named by its letter");
        assert!(read(&book, "book", Some("Missing"), &CsvOptions::default()).is_err());
        // The columns Add Delimited Text offers, found as the sheet is read.
        let c = columns(&book, None).unwrap();
        assert_eq!(c.names, ["name", "lon", "lat", "pop"]);
        assert_eq!((c.x.as_deref(), c.y.as_deref(), c.delimiter), (Some("lon"), Some("lat"), None));
        let c = columns(&book, Some("utm")).unwrap();
        assert_eq!((c.x.as_deref(), c.y.as_deref()), (Some("easting"), Some("northing")));
        assert_eq!(columns(&book, Some("Notes")).unwrap().names, ["text", "B"]);
        for cut in (0..book.len()).step_by(61) {
            let _ = read(&book[..cut], "t", None, &CsvOptions::default());
            let _ = columns(&book[..cut], None);
        }
    }

    #[test]
    fn junk_is_refused() {
        assert!(read(b"not a workbook", "t", None, &CsvOptions::default()).is_err());
        assert!(sheet_names(b"PK\x03\x04junk").is_err());
        // Two cells far apart are refused before calamine lays out the grid between them.
        let far = xlsx(&[("Far", &[&[r#"<c r="A1"><v>1</v></c>"#], &[r#"<c r="XFD1048576"><v>2</v></c>"#]])]);
        let e = read(&far, "t", None, &CsvOptions::default()).unwrap_err().to_string();
        assert!(e.contains("too large") && e.contains("XFD"), "{e}");
    }

    #[test]
    fn letters_count_from_column_a() {
        // The data starts in column C: a column without a heading is still named by its letter.
        let book = xlsx(&[(
            "S",
            &[
                &[r#"<c r="C1" t="inlineStr"><is><t>name</t></is></c>"#],
                &[r#"<c r="C2" t="inlineStr"><is><t>x</t></is></c>"#, r#"<c r="D2" t="inlineStr"><is><t>y</t></is></c>"#],
            ],
        )]);
        let (ds, _) = read(&book, "t", None, &CsvOptions::default()).unwrap();
        let names: Vec<&str> = ds.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["name", "D"]);
    }
}
