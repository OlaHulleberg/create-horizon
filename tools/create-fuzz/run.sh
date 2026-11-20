#!/usr/bin/env bash
# create-fuzz harness. Usage:
#   ./run.sh server            start the Java side (NeoForge dedicated server with Create) and wait until ready
#   ./run.sh stop              stop it
#   ./run.sh fuzz [args...]    the Rust driver (see `./run.sh fuzz help`)
# Expects a Create checkout next to this repository (`../Create` from the repository root, or CREATE_DIR),
# published with `./gradlew publishToMavenLocal`.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
export JAVA_HOME="${JAVA_HOME:-/usr/lib/jvm/java-21-openjdk-amd64}"
export FUZZ_FLUID_MANIFEST="$here/rust/src/fluids.json"
export FUZZ_TRANSPORT_MANIFEST="$here/rust/src/transport.json"
export FUZZ_MANIFEST="$here/rust/src/kinetics.json"
export FUZZ_DIR="${FUZZ_DIR:-$here/work}"
create_dir="${CREATE_DIR:-$here/../../../Create}"
gradlew="$create_dir/gradlew"
run="${FUZZ_GAME_DIR:-$here/java/run}"
export FUZZ_GAME_DIR="$run"
export FUZZ_HARNESS_HASH="$(cd "$here/java/src/main/java"; sha256sum dev/createfuzz/*.java | sha256sum | cut -d ' ' -f1)"
export FUZZ_COVERAGE_INCLUDES="${FUZZ_COVERAGE_INCLUDES:-com.simibubi.create.content.fluids.*:com.simibubi.create.content.kinetics.*:com.simibubi.create.content.logistics.depot.*:com.simibubi.create.content.logistics.funnel.*:com.simibubi.create.content.logistics.tunnel.*:com.simibubi.create.content.logistics.chute.*:com.simibubi.create.content.logistics.box.*:com.simibubi.create.api.stress.*:com.simibubi.create.api.registry.*}"
export FUZZ_COVERAGE_CACHE="${FUZZ_COVERAGE_CACHE:-$here/work/jacoco}"
export FUZZ_CREATE_JAR="${FUZZ_CREATE_JAR:-$HOME/.m2/repository/com/simibubi/create/create-1.21.1/6.0.11/create-1.21.1-6.0.11.jar}"
coverage_tools() {
    mkdir -p "$FUZZ_COVERAGE_CACHE/classes"
    local artifact expected url jar
    export FUZZ_COVERAGE_CLASSPATH="$FUZZ_COVERAGE_CACHE/classes"
    # SHA-1 values published by Maven Central at each pinned artifact's .jar.sha1 URL.
    while read -r artifact expected; do
        url="https://repo.maven.apache.org/maven2/$artifact.jar"
        jar="$FUZZ_COVERAGE_CACHE/${artifact##*/}.jar"
        if [ ! -f "$jar" ]; then
            if ! curl -fsSL "$url" -o "$jar"; then
                rm -f "$jar"
                return 1
            fi
        fi
        # Recheck cached jars too, before javac or a JVM can load any of them.
        if ! printf '%s  %s\n' "$expected" "$jar" | sha1sum --check --status; then
            echo "checksum mismatch: $jar (expected $expected); deleting" >&2
            rm -f "$jar"
            return 1
        fi
        FUZZ_COVERAGE_CLASSPATH="$FUZZ_COVERAGE_CLASSPATH:$jar"
    done <<'CHECKSUMS'
org/jacoco/org.jacoco.agent/0.8.14/org.jacoco.agent-0.8.14-runtime 4bb9b49d4e6c5b042fc7e6b4f1e3e808f7441dde
org/jacoco/org.jacoco.core/0.8.14/org.jacoco.core-0.8.14 5d317827447ab203bb90ecc7597850baae9c8565
org/ow2/asm/asm/9.9/asm-9.9 c29635c8a7afa03d74b33c1884df8abb2b3f3dcc
org/ow2/asm/asm-tree/9.9/asm-tree-9.9 f8de6eead6d24dd0f45bd065bbe112b2cda6ea21
org/ow2/asm/asm-commons/9.9/asm-commons-9.9 db9165a3bf908ded6b08612d583a15d1d0c7bda0
CHECKSUMS
    "$JAVA_HOME/bin/javac" -cp "$FUZZ_COVERAGE_CLASSPATH" -d "$FUZZ_COVERAGE_CACHE/classes" "$here/tools/coverage/CoverageReport.java"
    export FUZZ_COVERAGE_TOOL="$here/tools/coverage/report.sh"
}

case "${1:-}" in
server)
    coverage_tools
    export FUZZ_JACOCO="$FUZZ_COVERAGE_CACHE/org.jacoco.agent-0.8.14-runtime.jar=includes=$FUZZ_COVERAGE_INCLUDES,output=none,classdumpdir=$FUZZ_DIR/coverage-classes"
    mkdir -p "$run" "$FUZZ_DIR"
    # A local test server only: never public, no mobs, a flat world.
    echo "eula=true" > "$run/eula.txt"
    [ -f "$run/server.properties" ] || cat > "$run/server.properties" <<'P'
level-type=minecraft\:flat
generator-settings={"layers"\:[{"block"\:"minecraft\:bedrock","height"\:1}],"biome"\:"minecraft\:the_void"}
generate-structures=false
spawn-monsters=false
spawn-animals=false
spawn-npcs=false
online-mode=false
server-ip=127.0.0.1
server-port=25599
view-distance=4
simulation-distance=4
max-tick-time=-1
P
    sed -i "s/^server-port=.*/server-port=${FUZZ_PORT:-25599}/" "$run/server.properties"
    rm -f "$FUZZ_DIR/ready"
    (cd "$here/java"; exec setsid nohup "$gradlew" --no-daemon --max-workers=4 -q runServer </dev/null > "$FUZZ_DIR/server.log" 2>&1) &
    echo $! > "$FUZZ_DIR/server.pid"
    for _ in $(seq 600); do
        [ -f "$FUZZ_DIR/ready" ] && { echo "server ready"; exit 0; }
        # Exited (a zombie counts: nothing here may reap it).
        st="$(awk '{print $3}' "/proc/$(cat "$FUZZ_DIR/server.pid")/stat" 2>/dev/null || echo Z)"
        [ "$st" != Z ] || { echo "server exited; see $FUZZ_DIR/server.log"; exit 1; }
        sleep 1
    done
    echo "server not ready after 10 min"; exit 1 ;;
stop)
    # The Gradle process and the server JVM under it.
    if [ -f "$FUZZ_DIR/server.pid" ]; then
        pid="$(cat "$FUZZ_DIR/server.pid")"
        kids="$(pgrep -P "$pid" || true)"
        kill -- -"$pid" 2>/dev/null || kill $pid $kids 2>/dev/null || true
        for _ in $(seq 30); do kill -0 $pid $kids 2>/dev/null || break; sleep 1; done
        rm -f "$FUZZ_DIR/server.pid"
    fi ;;
fuzz)
    shift
    if [ "${1:-}" = gaps ]; then
        shift
        exec python3 "$here/tools/coverage/gaps.py" "$@"
    fi
    if [ "${1:-}" = cover ]; then coverage_tools; fi
    export FUZZ_FOUND="${FUZZ_FOUND:-$here/found}"
    manifest="$here/rust/Cargo.toml"
    # The checkout whose engine and block data the driver uses.
    export CREATE_HORIZON_DIR="$(cd "${CREATE_HORIZON:-$here/../..}" && pwd)"
    # CREATE_HORIZON=<checkout>: drive that checkout's engine instead of this one's. The driver is
    # copied beside it into its own folder (Cargo path dependencies are fixed in Cargo.toml).
    if [ -n "${CREATE_HORIZON:-}" ]; then
        engine="$(cd "$CREATE_HORIZON/create/src/engine" && pwd)"
        copy="${FUZZ_BUILD:-$FUZZ_DIR/builds}/$(echo "$engine" | md5sum | cut -c1-12)"
        mkdir -p "$copy"
        rm -rf "$copy/src"; cp -r "$here/rust/src" "$copy/src"; cp "$here/rust/Cargo.lock" "$copy/"
        sed "s#path = \"../../../create/src/engine\"#path = \"$engine\"#" "$here/rust/Cargo.toml" > "$copy/Cargo.toml"
        manifest="$copy/Cargo.toml"
        export CARGO_TARGET_DIR="$copy/target"
    fi
    if [ "${1:-}" = cover ]; then
        python3 "$here/tools/coverage/campaign.py" begin "$here" "$CREATE_HORIZON_DIR" "$create_dir" "$2"
    fi
    cargo +1.99.1 run -q --release --manifest-path "$manifest" -- "$@"
    if [ "${1:-}" = cover ]; then
        python3 "$here/tools/coverage/campaign.py" complete
    fi ;;
*)
    sed -n 2,8p "$0"; exit 1 ;;
esac
