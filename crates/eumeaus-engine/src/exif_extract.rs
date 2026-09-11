//! Pure, local extraction of EXIF metadata from an uploaded entity image
//! (SPEC.md §9.3's image upload, phase A of the EXIF-enrichment idea
//! tracked alongside issues #1/#20). No network call, no third party
//! sees anything — this is metadata already sitting in bytes the
//! investigator chose to upload, unlike a plugin scan.
//!
//! Named `exif_extract` rather than `exif` to avoid shadowing the
//! `kamadak-exif` crate's own `exif` name inside this module.
//!
//! [`extract`] never fails: a corrupt file, a format with no EXIF
//! segment (most PNGs, screenshots, GIFs), or a genuinely EXIF-less
//! JPEG are all the *normal* case, not an error — every field is
//! independently `Option`, and the whole struct is all-`None` when
//! nothing is found. Mirrors this project's "degrade, don't abort"
//! posture used for a bad plugin manifest or a bad sidecar config file.

use std::io::Cursor;

use exif::{In, Reader, Tag, Value};

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ExtractedMetadata {
    /// (latitude, longitude) in decimal degrees — negative for S/W.
    pub gps: Option<(f64, f64)>,
    /// `DateTimeOriginal` reformatted from EXIF's own
    /// `"YYYY:MM:DD HH:MM:SS"` ASCII form to `"YYYY-MM-DDTHH:MM:SS"`.
    /// Deliberately has **no timezone suffix**: EXIF's `DateTimeOriginal`
    /// is camera-local time with no offset recorded, so treating it as
    /// UTC would be dishonest. A GPS-timestamp-derived true-UTC value is
    /// a possible future refinement, not this phase.
    pub taken_at: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub software: Option<String>,
}

impl ExtractedMetadata {
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self == &ExtractedMetadata::default()
    }
}

pub(crate) fn extract(data: &[u8]) -> ExtractedMetadata {
    let Ok(exif) = Reader::new().read_from_container(&mut Cursor::new(data)) else {
        return ExtractedMetadata::default();
    };

    ExtractedMetadata {
        gps: gps_coordinates(&exif),
        taken_at: ascii_field(&exif, Tag::DateTimeOriginal)
            .and_then(|s| reformat_exif_datetime(&s)),
        camera_make: ascii_field(&exif, Tag::Make),
        camera_model: ascii_field(&exif, Tag::Model),
        software: ascii_field(&exif, Tag::Software),
    }
}

/// Reads a plain-text EXIF field (`Make`/`Model`/`Software`/
/// `DateTimeOriginal`) as its raw string content. Deliberately reads
/// `field.value` directly rather than `field.display_value()`: the
/// latter renders a generic `Value::Ascii` the way `{:?}` would (quoted,
/// e.g. `"Apple"`) — fine for a human-facing summary, wrong for a value
/// this code is about to store verbatim as an attribute.
fn ascii_field(exif: &exif::Exif, tag: Tag) -> Option<String> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    let Value::Ascii(ref components) = field.value else {
        return None;
    };
    let first = components.first()?;
    let text = String::from_utf8_lossy(first);
    let trimmed = text.trim_matches('\0').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Combines a GPS coordinate's three (degrees, minutes, seconds)
/// rationals into signed decimal degrees, negating for the S/W
/// hemisphere refs. `None` if either the coordinate or its hemisphere
/// ref is missing/malformed — a photo with only one of the two tags set
/// isn't a usable coordinate.
fn gps_coordinates(exif: &exif::Exif) -> Option<(f64, f64)> {
    let lat = dms_to_decimal_degrees(exif, Tag::GPSLatitude, Tag::GPSLatitudeRef, "S")?;
    let long = dms_to_decimal_degrees(exif, Tag::GPSLongitude, Tag::GPSLongitudeRef, "W")?;
    Some((lat, long))
}

fn dms_to_decimal_degrees(
    exif: &exif::Exif,
    coord_tag: Tag,
    ref_tag: Tag,
    negative_ref: &str,
) -> Option<f64> {
    let coord_field = exif.get_field(coord_tag, In::PRIMARY)?;
    let Value::Rational(ref rationals) = coord_field.value else {
        return None;
    };
    if rationals.len() != 3 {
        return None;
    }
    let degrees = rationals[0].to_f64();
    let minutes = rationals[1].to_f64();
    let seconds = rationals[2].to_f64();
    let magnitude = degrees + minutes / 60.0 + seconds / 3600.0;

    let ref_field = exif.get_field(ref_tag, In::PRIMARY)?;
    let sign = if ref_field.display_value().to_string().trim() == negative_ref {
        -1.0
    } else {
        1.0
    };
    Some(magnitude * sign)
}

/// EXIF's `DateTimeOriginal` uses `:` as the date separator too
/// (`"2024:05:01 10:30:00"`, not `"2024-05-01 10:30:00"`) — a
/// long-standing EXIF quirk, not a typo. Only the first two `:` (inside
/// the date portion, before the space) need replacing.
fn reformat_exif_datetime(raw: &str) -> Option<String> {
    let (date_part, time_part) = raw.split_once(' ')?;
    let mut date_fields = date_part.splitn(3, ':');
    let year = date_fields.next()?;
    let month = date_fields.next()?;
    let day = date_fields.next()?;
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    Some(format!("{year}-{month}-{day}T{time_part}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GPS_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/exif_gps.jpg");
    const GPS_SOUTH_EAST_FIXTURE: &[u8] =
        include_bytes!("../tests/fixtures/exif_gps_south_east.jpg");
    const NO_EXIF_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/no_exif.jpg");

    #[test]
    fn extracts_gps_camera_and_timestamp_from_a_tagged_jpeg() {
        let meta = extract(GPS_FIXTURE);

        let (lat, long) = meta.gps.expect("fixture has GPS tags");
        assert!((lat - 40.689247).abs() < 1e-4, "lat was {lat}");
        assert!((long - -74.044502).abs() < 1e-4, "long was {long}");

        assert_eq!(meta.camera_make.as_deref(), Some("Apple"));
        assert_eq!(meta.camera_model.as_deref(), Some("iPhone 14 Pro"));
        assert_eq!(meta.software.as_deref(), Some("17.4.1"));
        assert_eq!(meta.taken_at.as_deref(), Some("2024-05-01T10:30:00"));
    }

    #[test]
    fn a_jpeg_with_no_exif_segment_returns_all_none_not_an_error() {
        let meta = extract(NO_EXIF_FIXTURE);
        assert!(meta.is_empty(), "expected no metadata, got {meta:?}");
    }

    #[test]
    fn garbage_bytes_return_all_none_not_a_panic() {
        let meta = extract(b"this is not an image at all");
        assert!(meta.is_empty());
    }

    #[test]
    fn southern_and_eastern_hemispheres_negate_correctly() {
        // Sydney Opera House: S latitude (negative), E longitude
        // (positive) — the other fixture only covers N/W, so this
        // exercises the opposite sign on each axis independently.
        let meta = extract(GPS_SOUTH_EAST_FIXTURE);
        let (lat, long) = meta.gps.expect("fixture has GPS tags");
        assert!(lat < 0.0, "southern latitude must be negative, was {lat}");
        assert!((lat - -33.856784).abs() < 1e-4, "lat was {lat}");
        assert!(long > 0.0, "eastern longitude must be positive, was {long}");
        assert!((long - 151.215297).abs() < 1e-4, "long was {long}");
    }

    #[test]
    fn reformat_exif_datetime_rejects_a_malformed_string() {
        assert_eq!(
            reformat_exif_datetime("2024:05:01 10:30:00").as_deref(),
            Some("2024-05-01T10:30:00")
        );
        assert_eq!(reformat_exif_datetime("not a valid exif datetime"), None);
    }
}
