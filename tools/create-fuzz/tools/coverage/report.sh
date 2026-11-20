#!/usr/bin/env bash
set -euo pipefail
exec "$JAVA_HOME/bin/java" -cp "$FUZZ_COVERAGE_CLASSPATH" CoverageReport "$1" "$2" "$FUZZ_CREATE_JAR" "$FUZZ_COVERAGE_INCLUDES" "$3" "$FUZZ_DIR/coverage-classes"
