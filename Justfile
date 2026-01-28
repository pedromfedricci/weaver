log := "progress.log"
csv := "output.csv"

default:
    @just --list

[private]
clean-output:
    rm -f {{log}} {{csv}}

# Build release binary.
build:
    cargo build --release

# Build debug binary.
build-debug:
    cargo build

# Run all checks (fmt, clippy, test).
check: fmt-check clippy test

# Check formatting.
fmt-check:
    cargo fmt --check

# Format code.
fmt:
    cargo fmt

# Run clippy.
clippy:
    cargo clippy --all-targets -- \
        -D warnings -D clippy::pedantic -D clippy::nursery

# Run tests.
test:
    cargo test

# Clean build artifacts and output files.
clean:
    cargo clean
    rm -f {{log}} {{csv}}

# Export CSV from progress log.
export-csv:
    cargo run --release -- --export

# Run with file (fresh=true clears progress, else resumes).
run file fresh="true":
    {{ if fresh == "true" { "just clean-output" } else { "" } }}
    cat {{file}} | cargo run --release

# Run with few.txt (fresh=true clears progress, else resumes).
run-few fresh="true": (run "websites/few.txt" fresh)

# Run with many.txt (fresh=true clears progress, else resumes).
run-many fresh="true": (run "websites/many.txt" fresh)

# Debug run with domain (fresh=true clears progress, else resumes).
debug domain="github.com" level="debug" timeout="20" fresh="true": build-debug
    {{ if fresh == "true" { "just clean-output" } else { "" } }}
    echo "{{domain}}" | RUST_LOG="weaver={{level}}" timeout {{timeout}} \
        target/debug/weaver --max-pages 2 --max-depth 1 2>&1

# Debug run with file (fresh=true clears progress, else resumes).
debug-file file level="debug" timeout="60" fresh="true": build-debug
    {{ if fresh == "true" { "just clean-output" } else { "" } }}
    cat {{file}} | RUST_LOG="weaver={{level}}" timeout {{timeout}} \
        target/debug/weaver --max-pages 2 --max-depth 1 2>&1

# Debug run with websites/few.txt (fresh=true clears progress, else resumes).
debug-few fresh="true": (debug-file "websites/few.txt" fresh)

# Count domains in progress log.
stats file=log:
    @echo "Started:    $(grep -c '^STARTED' {{file}} 2>/dev/null || echo 0)"
    @echo "Completed:  $(grep -c '^COMPLETED' {{file}} 2>/dev/null || echo 0)"
    @echo "Logo found: $(grep -c 'LOGO_FOUND' {{file}} 2>/dev/null || echo 0)"
    @echo "No logo:    $(grep -c 'NO_LOGO' {{file}} 2>/dev/null || echo 0)"
    @echo "Limit:      $(grep -c 'LIMIT_REACHED' {{file}} 2>/dev/null || echo 0)"

# Watch progress log.
watch-log file=log:
    tail -f {{file}}
