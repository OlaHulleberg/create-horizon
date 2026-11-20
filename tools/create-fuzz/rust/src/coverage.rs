//! Coverage collection consumes isolated execution data and measures the union of branch edges.
use std::{
    path::{Path, PathBuf},
    process::Command,
};
pub struct Collector {
    tool: String,
    report: PathBuf,
    merged: PathBuf,
}
impl Collector {
    pub fn new(report: &Path) -> Self {
        let merged = report.join("union.exec");
        if merged.exists() {
            std::fs::remove_file(&merged).unwrap();
        }
        Self {
            tool: std::env::var("FUZZ_COVERAGE_TOOL").expect("use run.sh fuzz cover"),
            report: report.into(),
            merged,
        }
    }
    pub fn measure(&mut self, exec: &Path) -> usize {
        let output = Command::new(&self.tool)
            .args([
                exec.as_os_str(),
                self.merged.as_os_str(),
                self.report.as_os_str(),
            ])
            .output()
            .expect("coverage analysis");
        assert!(
            output.status.success(),
            "coverage analysis: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }
}
