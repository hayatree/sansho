# Reference simulator used only as a black-box oracle for result comparison.
# ngspice is invoked as an external program; its sources are never read or linked.
FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ngspice \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /work
ENTRYPOINT ["ngspice"]
