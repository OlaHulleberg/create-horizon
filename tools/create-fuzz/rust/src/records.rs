//! Optional streaming compression keeps complete triage records without large JSON files.
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

pub fn reader(path: &Path) -> Box<dyn Read> {
    let file = BufReader::new(std::fs::File::open(path).expect("record file"));
    if path.extension().is_some_and(|ext| ext == "gz") {
        Box::new(GzDecoder::new(file))
    } else {
        Box::new(file)
    }
}

pub fn write(path: &Path, value: &impl serde::Serialize, compressed: bool) {
    let output = if compressed {
        path.with_extension("json.gz")
    } else {
        path.to_path_buf()
    };
    let file = BufWriter::new(std::fs::File::create(output).expect("create record"));
    if compressed {
        let mut encoder = GzEncoder::new(file, Compression::fast());
        serde_json::to_writer(&mut encoder, value).expect("write compressed record");
        encoder
            .finish()
            .expect("finish compressed record")
            .flush()
            .expect("flush record");
    } else {
        let mut writer = file;
        serde_json::to_writer_pretty(&mut writer, value).expect("write record");
        writer.flush().expect("flush record");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_records_round_trip_in_both_formats() {
        let dir = std::env::temp_dir().join(format!("fuzz-records-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let value = serde_json::json!({"scenario":{"seed":42},"java":vec!["water"; 1000],"diffs":["field"]});
        for compressed in [false, true] {
            let path = dir.join("record.json");
            write(&path, &value, compressed);
            let output = if compressed {
                path.with_extension("json.gz")
            } else {
                path
            };
            let actual: serde_json::Value = serde_json::from_reader(reader(&output)).unwrap();
            assert_eq!(actual, value);
        }
        assert!(
            std::fs::metadata(dir.join("record.json.gz")).unwrap().len()
                < std::fs::metadata(dir.join("record.json")).unwrap().len()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
