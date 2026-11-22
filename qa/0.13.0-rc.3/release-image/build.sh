#!/bin/sh
# Builds, from the exported branch head, the checks image and the release
# candidate as .github/workflows/image.yml does on a tag (channel release),
# then the probe build of the same tree (qa/0.13.0-rc.3/Dockerfile.probe).
set -u
CT=/usr/local/bin/container
SHA=$(cat ~/Projects/scratch/rc3-release/sha)
SRC=~/Projects/scratch/rc3-release/src-$SHA
LOGS=~/Projects/worktrees/hotline-computer/rc3-review-fixes/qa/0.13.0-rc.3/release-image
TAG=hotline-computer:0.13.0-rc.3-relcheck
cd "$SRC" || exit 1
stamp() { date -u +%Y-%m-%dT%H:%M:%SZ; }
step() {
  name=$1; shift
  echo "# $(stamp) $name: $CT build $*" > "$LOGS/build-$name.log"
  "$CT" build --progress plain -c 8 -m 16g "$@" >> "$LOGS/build-$name.log" 2>&1
  code=$?
  echo "# $(stamp) $name: exit $code" >> "$LOGS/build-$name.log"
  echo "$(stamp) $name exit $code" >> "$LOGS/builds.txt"
  return $code
}
echo "$(stamp) start, commit $SHA, context $SRC (git archive of the commit)" >> "$LOGS/builds.txt"
step checks --build-arg HOTLINE_BUILD_REVISION=$SHA --build-arg HOTLINE_BUILD_CHANNEL=release --target checks -t $TAG-checks . || exit 1
step release --build-arg HOTLINE_BUILD_REVISION=$SHA --build-arg HOTLINE_BUILD_CHANNEL=release -t $TAG . || exit 1
step probe -f qa/0.13.0-rc.3/Dockerfile.probe --build-arg BASE=$TAG --build-arg HOTLINE_BUILD_REVISION=$SHA --build-arg HOTLINE_BUILD_CHANNEL=release -t $TAG-probe . || exit 1
echo "$(stamp) all built" >> "$LOGS/builds.txt"
