# Pre-built binaries are injected into binaries/${TARGETARCH}/lemma by CI.
# See .github/workflows/release.yml publish-docker job.
# CA bundle required: lemma mcp builds reqwest/rustls with system roots at startup.
FROM alpine:3.21 AS certs
RUN apk add --no-cache ca-certificates

FROM scratch
COPY --from=certs /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
ARG TARGETARCH
COPY binaries/${TARGETARCH}/lemma /usr/local/bin/lemma
ENTRYPOINT ["/usr/local/bin/lemma"]
CMD ["--help"]
EXPOSE 8012
