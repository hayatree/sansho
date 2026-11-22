# Provenance: original container recipe for the Sansho verification harness.
# Reference tools used only as black-box oracles for result comparison.
# ngspice and OpenVAF-Reloaded are invoked as external programs; their sources are never read,
# linked or vendored. OpenVAF is used solely to compile user-supplied Verilog-A models to OSDI
# so that ngspice can produce reference results for sansho-va.
FROM debian:trixie-slim
ARG OPENVAF_VERSION=v24.0.2mob
RUN apt-get update \
 && apt-get install -y --no-install-recommends ngspice libllvm18 gcc libc6-dev ca-certificates curl \
 && curl -fsSL "https://github.com/OpenVAF/OpenVAF-Reloaded/releases/download/${OPENVAF_VERSION}/openvaf-r-${OPENVAF_VERSION}-linux-x86_64.tar.gz" \
      | tar -xz -C /opt \
 && ln -s "/opt/openvaf-r-${OPENVAF_VERSION}-linux-x86_64/bin/openvaf-r" /usr/local/bin/openvaf-r \
 && apt-get purge -y curl && apt-get autoremove -y \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /work
ENTRYPOINT ["ngspice"]
