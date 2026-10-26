# Release maintenance

## Build outputs

The `Release` workflow runs on pull requests, version tags, and manual dispatch.
It does not repeat the same checks on every merge to `main`. Manual dispatch
checks builds only; publication requires a `v*` tag whose version matches
Cargo.toml and whose commit is in the history of `main`.

Each native AMD64/ARM64 runner tests the musl target, builds one release
binary, and packages it using the runtime-only Dockerfile. A SHA-256 comparison
checks that the container contains the exact release binary. The container
then passes a startup/HTTP smoke test before any release job can run.

For tagged releases, the tested images are transferred as Docker archives.
The publishing job loads and pushes them without rebuilding. It publishes
architecture tags (`VERSION-amd64`, `VERSION-arm64`), a common `VERSION` image
index, and the moving `latest` alias.

GitHub Release assets:

- `sub-injector-linux-x86_64`
- `sub-injector-linux-aarch64`
- `config.toml.example`
- `docker-compose.yml.example`
- `container-image.txt` — common multi-platform image reference pinned by digest
- `SHA256SUMS` — checksums of the five assets above

## Publishing a version

1. Update Cargo.toml, Cargo.lock, pinned installation examples in both READMEs
   and Compose, and the changelog together in a reviewed PR.
2. Wait for both `Build and test amd64` and `Build and test arm64` to pass.
3. Release one version at a time. Merge the PR. With the owner's approval, create and push `vVERSION` on the
   merged commit. Do not reuse an existing published version.
4. The workflow publishes images, uploads all assets to a draft release,
   checks the asset list, updates `latest`, and publishes the draft last.
5. Verify anonymous image access, both platforms, and release checksums.

For a failed run before publication, fix the cause and rerun only if the same
version and source are still appropriate. Assets can be completed while the
release remains a draft. An interrupted publication may leave architecture
image tags or the common version tag in GHCR. Once the release is published,
the workflow refuses to republish it: fixes require a new version.

## Proposed repository settings (not applied by this change)

- **Main ruleset:** require a PR, both architecture checks, resolved review
  conversations, and no force pushes or deletion. Use squash merges for a
  single coherent commit. Require zero reviewer approvals while there is only
  one maintainer; otherwise the owner cannot approve their own PR. Enable the
  ruleset after the check names have run successfully in the new workflow.
- **Version tag ruleset (`v*`):** prevent updates and deletion; permit creation
  by the release maintainer. Choose bypass permissions deliberately rather
  than silently exempting all administrators.
- **Release immutability:** enable before the next release, after confirming
  draft publication works. This protects future published release assets and
  associated Git tags; it does not retroactively protect existing releases.
- **Merge settings:** allow squash merges and delete merged PR branches.
- **Actions:** keep the default token read-only and PR approval disabled.
  Only the publishing job receives `contents: write` and `packages: write`.
- Keep Dependabot security updates, secret scanning and push protection enabled.

GitHub release immutability does not lock GHCR tags. For deployments requiring
an exact image, use the `ghcr.io/...@sha256:...` reference from
`container-image.txt`; it selects the same multi-platform index by content.
The version tag is convenient, while `latest` intentionally moves. The
workflow prevents republishing an already published release, but registry
write permissions still need to be controlled separately.

Official references: [immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases),
[enabling immutability](https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/establish-provenance-and-integrity/prevent-release-changes),
[repository rulesets](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/available-rules-for-rulesets).
