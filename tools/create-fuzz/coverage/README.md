# Reviewed branch explanations

`subsystems.json` defines membership by slash-separated Java package prefix. Belts
are deliberately also counted in kinetics; depot classes belong to belts. Adding
fluids requires a new membership entry and reviewed rules, without reporter changes.

`reasons.tsv` uses exact JVM class and method signatures (`*` means the whole
class), inclusive source line bounds, a category, an explanation and a SHA-256 of
the reviewed Create Java source. Explanations currently cover only the reviewed
entries in that file. Unreviewed entries remain UNCLASSIFIED and fail the command;
there is no fallback that labels reachable gameplay out of scope.

Run `./run.sh fuzz gaps [kinetics|belts] --provenance campaign.json` after a coverage
campaign. By default it reads `$FUZZ_COVERAGE_REPORT/never-reached.tsv`, or
`$FUZZ_DIR/coverage/never-reached.tsv`, and writes `docs/coverage/*.md`. The sibling
`classes.tsv` is required to detect missing package denominators. Provenance JSON
must contain `fuzzer`, `engine`, `create` (full commits) and `command`. The reports, and
the campaign's input TSVs and provenance they are built from, are generated and not tracked
(see `.gitignore`). Keep a campaign's inputs beside its reports to regenerate them offline:

```
./run.sh fuzz gaps --input docs/coverage/input/never-reached.tsv --provenance docs/coverage/input/campaign.json
```

Missing explanations, ambiguous rules and missing package denominators fail.
Report counts are branch outcomes, not method counts or inferred edge identities.
The source hash is review evidence; refreshing reasons after a Create upgrade
requires checking their source and replacing the hash. The reporter never reads
Java sources or dependencies.

A method selector may also be a Java method name without its descriptor, covering
all overloads. Narrower line ranges override wider ranges; at equal ranges a
method selector overrides `*`. Equal-priority matches are an error. This permits
mixed server/client classes to keep explicit line-level client guards without
hiding their server gameplay gaps.

`fuzz cover` writes `campaign.json` automatically and marks it complete only after
the driver succeeds. `fuzz gaps` checks completion and the input hash. An engine
source archive must set `FUZZ_ENGINE_REVISION` to its archived commit. Preserve the
initial corpus along with the report to reproduce a guided continuation; its
filenames and contents are hashed in the campaign header. The isolated-server
settings `FUZZ_PORT`, `FUZZ_DIR`, `FUZZ_GAME_DIR`, `FUZZ_JAVA_BUILD` and
`FUZZ_COVERAGE_CACHE` keep its files apart from the shared server.


A line group can include several missing outcomes. When a never-called guard has
both a valid gameplay edge and a defensive/dead edge, its primary category follows
the missing gameplay/client input and its explanation names the defensive/dead
edge too. The report does not claim every outcome on that line is impossible.
Static connection-state injection is already generated for the kinetic catalogue;
a missing machine workflow means missing processing, entity or player inputs,
not absence of that block from all generated layouts.
