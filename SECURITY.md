# Security

## Reporting a vulnerability

Email **security@blackrainbowlabs.com** with the details. Do not open a public issue for it.

| When | What you get |
|---|---|
| within five working days | a first reply that says whether the report is understood and can be reproduced |
| within fifteen working days | an assessment: the impact, whether a fix is coming, and when |
| the next release | the fix |

## What is in scope

- **The index host.** It listens on a loopback port and refuses any request without the token in its
  host file. A way for another user on the machine to query it, or to make it read outside its
  checkout, is in scope.
- **The files it reads.** A file in a checkout that makes the index crash, loop, or allocate without
  limit.
- **The installers.** `install.sh` and `install.ps1` check every download against the published
  `SHA256SUMS`, which is signed with minisign. A way around that check is in scope.

`docs/architecture.md` describes the host's security model.
