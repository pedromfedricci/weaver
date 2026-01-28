# Weaver

A logo extraction tool that crawls domains and outputs a CSV mapping of domain
to logo url and logo hash (learning project).

## How

Feed it a list of domains via stdin, it crawls each one looking for logos (icons,
og:image, img tags with "logo" in the attributes), downloads them, hashes them,
and writes results to a progress log. The CSV is derived from the log.

Crash recovery by replaying progress in a append-only log, so you can resume
from previous runs.

## Architecture

### Single process, three main components:

```
stdin -> [Async Executor] <-> [Coordinator] <-> [Worker Pool]
                |                   |                 |
                -> http fetch       -> log progress   -> html parsing
                                                      -> logo heuristics
                                                      -> logo hashing
```

**Async Executor** (multi-threaded tokio): Handles HTTP file fetching, HTML and images.
I/O-bound work that benefits from async. Manages concurrency via semaphore, deduplicates
requests.

**Coordinator** (dedicated OS thread): Bridges async and sync tasks. Tracks
per-domain state, enforces crawl limits, writes the progress log and CSV output.

**Worker Pool** (OS thread pool): CPU-bound work: HTML parsing, link extraction,
logo heuristics, hashing. Stateless workers pull from a shared queue (bounded channel).

### Progress recovery on startup:

```
progress.log -> CrawlState { completed, in_progress } -> CSVWriter -> output.csv
                                     |
                +--------------------+--------------------+
                |                                         |
                v                                         v
        [Async Executor]                            [Coordinator]
        (re-queue pending paths)                    (append found paths)
```

On startup, the progress log is parsed to reconstruct state:
- **Completed domains**: Skip entirely (already have logo or exhausted).
- **Queued paths**: Paths discovered but not yet fetched, re-queued for fetching.
- **Searched paths**: Already fetched, used for deduplication.

## Usage

### Run:

```bash
cat websites/few.txt | cargo run --release
```

### Resume after interruption:

```bash
# skips completed domains and searched paths, resumes non-searched paths.
cat websites/few.txt | cargo run --release
```

### Custom configuration:

```bash
cat websites/few.txt | cargo run --release -- \
    --concurrency 200 \
    --max-pages 100 \
    --max-depth 5 \
    --workers 8
```

### For debug info, add or export:

```bash
RUST_LOG=weaver=debug
```

## Output

`progress.log`: Append-only log of crawl state.
`output.csv`: Per domain logo paths and hashes.


## Development

### With Nix flake + direnv (recommended)

> **Note**: Nix flakes are an experimental feature. You need to enable them by adding
> `experimental-features = nix-command flakes` to your `~/.config/nix/nix.conf`.

If you have [Nix] and [direnv] installed:

```bash
# In the project root, only once is needed.
direnv allow
```

This automatically loads the dev shell.

### With Nix flake only

```bash
# Loads environment on demand.
nix develop
```

### Build with Nix

```bash
# Build with nix.
nix build
# Run executable from nix store.
cat websites/few.txt | nix run
```

[Nix]: https://nixos.org/ 
[direnv]: https://direnv.net/
