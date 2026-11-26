# Provenance: original container recipe for the Sansho verification harness.
# Reference tools used only as black-box oracles for result comparison.
# ngspice and OpenVAF-Reloaded are invoked as external programs; their sources are never read,
# linked or vendored. OpenVAF is used solely to compile user-supplied Verilog-A models to OSDI
# so that ngspice can produce reference results for sansho-va.
FROM debian:trixie-slim
ARG OPENVAF_VERSION=v24.0.2mob
# SHA-256 of openvaf-r-${OPENVAF_VERSION}-linux-x86_64.tar.gz, as published in the GitHub
# release asset metadata. Update it together with OPENVAF_VERSION.
ARG OPENVAF_SHA256=b12b7b1726d103e18c2588c3a1f91f8475cf03df8a3bde14d919797d221e3110
RUN apt-get update \
 && apt-get install -y --no-install-recommends ngspice libllvm18 gcc libc6-dev ca-certificates curl \
 && curl -fsSL -o /tmp/openvaf-r.tar.gz "https://github.com/OpenVAF/OpenVAF-Reloaded/releases/download/${OPENVAF_VERSION}/openvaf-r-${OPENVAF_VERSION}-linux-x86_64.tar.gz" \
 && echo "${OPENVAF_SHA256}  /tmp/openvaf-r.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/openvaf-r.tar.gz -C /opt \
 && rm /tmp/openvaf-r.tar.gz \
 && ln -s "/opt/openvaf-r-${OPENVAF_VERSION}-linux-x86_64/bin/openvaf-r" /usr/local/bin/openvaf-r \
 && apt-get purge -y curl && apt-get autoremove -y \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /work
ENTRYPOINT ["ngspice"]
