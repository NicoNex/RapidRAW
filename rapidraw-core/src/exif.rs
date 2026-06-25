//! Full photo-metadata extraction, shared by both UIs (ported from src-tauri's
//! `exif_processing::extract_metadata`). Reads standard EXIF via `kamadak-exif`,
//! and falls back to `rawler`'s decoder metadata for RAW files that carry no
//! container EXIF — so RAW shots still show camera/lens/exposure/GPS.
//!
//! Tauri-free: a `name -> display string` map. Dates are kept as the raw EXIF
//! string (the Tauri build additionally normalizes them via chrono; not worth a
//! core dependency here).

use std::collections::BTreeMap;

use exif::Exif;

/// Collapse very long values (e.g. embedded maker notes) so the UI stays sane.
pub fn truncate_large_exif(value: &str) -> String {
    if value.len() <= 500 {
        return value.to_string();
    }
    let mut start_idx = 200;
    while !value.is_char_boundary(start_idx) {
        start_idx -= 1;
    }
    let mut end_idx = value.len() - 200;
    while !value.is_char_boundary(end_idx) {
        end_idx += 1;
    }
    if start_idx < end_idx {
        return format!("{}...{}", &value[..start_idx], &value[end_idx..]);
    }
    value.to_string()
}

fn read_exif(file_bytes: &[u8]) -> Option<Exif> {
    let exifreader = exif::Reader::new();
    let mut cursor = std::io::Cursor::new(file_bytes);
    exifreader.read_from_container(&mut cursor).ok()
}

fn read_raw_metadata(file_bytes: &[u8]) -> Option<rawler::decoders::RawMetadata> {
    let loader = rawler::RawLoader::new();
    let raw_source = rawler::rawsource::RawSource::new_from_slice(file_bytes);
    let decoder = loader.get_decoder(&raw_source).ok()?;
    decoder.raw_metadata(&raw_source, &Default::default()).ok()
}

/// Read all readable metadata as `name -> display value`. Empty if nothing is
/// readable. Standard EXIF wins; the rawler fallback only runs when the
/// container has no EXIF at all (matching the Tauri behaviour).
pub fn extract_metadata(file_bytes: &[u8]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();

    if let Some(exif_obj) = read_exif(file_bytes) {
        for field in exif_obj.fields() {
            match field.tag {
                exif::Tag::ExposureTime => {
                    if let exif::Value::Rational(ref v) = field.value
                        && !v.is_empty()
                    {
                        let r = &v[0];
                        if r.num == 1 && r.denom > 1 {
                            map.insert("ExposureTime".into(), format!("1/{} s", r.denom));
                        } else {
                            let val = r.num as f32 / r.denom as f32;
                            if val < 1.0 && val > 0.0 {
                                map.insert(
                                    "ExposureTime".into(),
                                    format!("1/{} s", (1.0 / val).round()),
                                );
                            } else {
                                map.insert("ExposureTime".into(), format!("{val} s"));
                            }
                        }
                    }
                }
                exif::Tag::FNumber => {
                    if let exif::Value::Rational(ref v) = field.value
                        && !v.is_empty()
                    {
                        let val = v[0].num as f32 / v[0].denom as f32;
                        map.insert("FNumber".into(), format!("f/{val}"));
                    }
                }
                exif::Tag::FocalLength => {
                    if let exif::Value::Rational(ref v) = field.value
                        && !v.is_empty()
                    {
                        let val = v[0].num as f32 / v[0].denom as f32;
                        map.insert("FocalLength".into(), val.to_string());
                        map.insert("FocalLengthIn35mmFilm".into(), val.to_string());
                    }
                }
                exif::Tag::PhotographicSensitivity | exif::Tag::ISOSpeed => {
                    map.insert(
                        "PhotographicSensitivity".into(),
                        field.display_value().to_string(),
                    );
                    map.insert("ISOSpeed".into(), field.display_value().to_string());
                }
                exif::Tag::LensModel => {
                    map.insert(
                        "LensModel".into(),
                        field.display_value().to_string().replace('"', ""),
                    );
                }
                exif::Tag::DateTimeOriginal => {
                    map.insert("DateTimeOriginal".into(), field.display_value().to_string());
                }
                _ => {
                    let val = field.display_value().with_unit(&exif_obj).to_string();
                    if !val.trim().is_empty() {
                        map.entry(field.tag.to_string()).or_insert(val);
                    }
                }
            }
        }
    }

    if !map.is_empty() {
        return map;
    }

    // No container EXIF (typical for some RAW): fall back to rawler's decoder.
    let Some(metadata) = read_raw_metadata(file_bytes) else {
        return map;
    };
    let exif = metadata.exif;

    let fmt_rat = |r: &rawler::formats::tiff::Rational| -> f32 {
        if r.d == 0 { 0.0 } else { r.n as f32 / r.d as f32 }
    };
    let fmt_srat = |r: &rawler::formats::tiff::SRational| -> f32 {
        if r.d == 0 { 0.0 } else { r.n as f32 / r.d as f32 }
    };
    let put = |map: &mut BTreeMap<String, String>, key: &str, val: String| {
        let t = val.trim();
        if !t.is_empty() {
            map.insert(key.into(), truncate_large_exif(t));
        }
    };

    put(&mut map, "Make", metadata.make.clone());
    put(&mut map, "Model", metadata.model.clone());
    if let Some(v) = exif.artist.clone() {
        put(&mut map, "Artist", v);
    }
    if let Some(v) = exif.copyright.clone() {
        put(&mut map, "Copyright", v);
    }
    if let Some(v) = exif.user_comment.clone() {
        put(&mut map, "UserComment", v);
    }
    if let Some(v) = exif.date_time_original.clone() {
        put(&mut map, "DateTimeOriginal", v);
    }
    if let Some(v) = exif.create_date.clone() {
        put(&mut map, "CreateDate", v);
    }
    if let Some(v) = exif.lens_model.clone() {
        put(&mut map, "LensModel", v);
    } else if let Some(lens) = &metadata.lens {
        put(&mut map, "LensModel", lens.lens_model.clone());
    }
    if let Some(v) = exif.lens_make.clone() {
        put(&mut map, "LensMake", v);
    }
    if let Some(r) = exif.fnumber {
        put(&mut map, "FNumber", format!("f/{}", fmt_rat(&r)));
    }
    if let Some(r) = exif.exposure_time {
        let s = if r.n == 1 && r.d > 1 {
            format!("1/{} s", r.d)
        } else {
            let val = fmt_rat(&r);
            if val < 1.0 && val > 0.0 {
                format!("1/{} s", (1.0 / val).round())
            } else {
                format!("{val} s")
            }
        };
        put(&mut map, "ExposureTime", s);
    }
    if let Some(r) = exif.shutter_speed_value {
        put(&mut map, "ShutterSpeedValue", fmt_srat(&r).to_string());
    }
    if let Some(v) = exif.iso_speed {
        put(&mut map, "PhotographicSensitivity", v.to_string());
    } else if let Some(v) = exif.iso_speed_ratings {
        put(&mut map, "PhotographicSensitivity", v.to_string());
    }
    if let Some(r) = exif.focal_length {
        let val = fmt_rat(&r);
        put(&mut map, "FocalLength", val.to_string());
        put(&mut map, "FocalLengthIn35mmFilm", val.to_string());
    }
    if let Some(v) = exif.orientation {
        put(&mut map, "Orientation", v.to_string());
    }
    if let Some(v) = exif.flash {
        put(&mut map, "Flash", v.to_string());
    }
    if let Some(v) = exif.white_balance {
        put(&mut map, "WhiteBalance", v.to_string());
    }
    if let Some(v) = exif.color_space {
        put(&mut map, "ColorSpace", v.to_string());
    }

    if let Some(gps) = exif.gps {
        let coord = |c: &[rawler::formats::tiff::Rational; 3]| {
            format!("{} deg {} min {} sec", fmt_rat(&c[0]), fmt_rat(&c[1]), fmt_rat(&c[2]))
        };
        if let Some(lat) = gps.gps_latitude {
            put(&mut map, "GPSLatitude", coord(&lat));
        }
        if let Some(r) = gps.gps_latitude_ref {
            put(&mut map, "GPSLatitudeRef", r);
        }
        if let Some(lon) = gps.gps_longitude {
            put(&mut map, "GPSLongitude", coord(&lon));
        }
        if let Some(r) = gps.gps_longitude_ref {
            put(&mut map, "GPSLongitudeRef", r);
        }
        if let Some(alt) = gps.gps_altitude {
            put(&mut map, "GPSAltitude", format!("{} m", fmt_rat(&alt)));
        }
    }

    map
}
