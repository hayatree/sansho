# Provenance: original container recipe for the Sansho verification harness.
# Builds an ngspice 44.2 executable with the experimental PSS analysis enabled
# (configure option --enable-pss, ngspice 44 manual §28.1.8), used only as a
# black-box oracle for .pss results.
#
# Isolation rules (see README.md):
# - The tarball is checked against the pinned NGSPICE_SHA256 before it is
#   extracted; a mismatch fails the build.
# - The source tarball is downloaded, built and deleted inside ONE RUN step of
#   the builder stage, so no image layer or build-cache entry contains source.
# - Compiler, configure and make output goes to /dev/null; only a one-line
#   status is printed. Nobody reads the build log.
# - The runtime stage copies only the installed executable, code models and
#   init scripts. Headers, man pages and sources are not copied.
FROM debian:trixie-slim AS build
ARG NGSPICE_VERSION=44.2
# Expected SHA-256 of the ngspice-${NGSPICE_VERSION}.tar.gz release tarball. This is the
# digest of the reviewed tarball recorded in README.md ("Recorded build"). Update it
# together with NGSPICE_VERSION; the build fails if the download does not match.
ARG NGSPICE_SHA256=e7dadfb7bd5474fd22409c1e5a67acdec19f77e597df68e17c5549bc1390d7fd
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential bison flex ca-certificates curl \
      libreadline-dev libfftw3-dev \
 && rm -rf /var/lib/apt/lists/*
RUN set -eu; \
    src=/tmp/ngspice-src; mkdir -p "$src"; cd "$src"; \
    curl -fsSL -o ngspice.tar.gz \
      "https://downloads.sourceforge.net/project/ngspice/ng-spice-rework/old-releases/${NGSPICE_VERSION}/ngspice-${NGSPICE_VERSION}.tar.gz"; \
    echo "${NGSPICE_SHA256}  ngspice.tar.gz" | sha256sum -c - >/dev/null \
      || { echo "ngspice-pss build: FAILED tarball SHA-256 mismatch"; cd /; rm -rf "$src"; exit 1; }; \
    sha=$(sha256sum ngspice.tar.gz | cut -d' ' -f1); \
    tar -xzf ngspice.tar.gz; rm ngspice.tar.gz; \
    cd "ngspice-${NGSPICE_VERSION}"; \
    flags="--prefix=/opt/ngspice-pss --enable-pss --enable-cider --disable-debug --without-x"; \
    status=ok; \
    ./configure $flags >/dev/null 2>&1 || status="configure exit $?"; \
    if [ "$status" = ok ]; then make -j"$(nproc)" >/dev/null 2>&1 || status="make exit $?"; fi; \
    if [ "$status" = ok ]; then make install >/dev/null 2>&1 || status="make install exit $?"; fi; \
    cd /; rm -rf "$src"; \
    if [ "$status" != ok ]; then echo "ngspice-pss build: FAILED at $status (log withheld by policy)"; exit 1; fi; \
    rm -rf /opt/ngspice-pss/include /opt/ngspice-pss/share/man /opt/ngspice-pss/share/info; \
    printf 'ngspice %s\ntarball sha256 %s\nconfigure %s\nbuilt %s\n' \
      "$NGSPICE_VERSION" "$sha" "$flags" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > /opt/ngspice-pss/BUILDINFO; \
    echo "ngspice-pss build: ok"

FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends libreadline8t64 libfftw3-double3 libgomp1 libstdc++6 \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /opt/ngspice-pss /opt/ngspice-pss
WORKDIR /work
ENTRYPOINT ["/opt/ngspice-pss/bin/ngspice"]
