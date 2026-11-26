# Security policy

## Supported versions

Only the latest release gets security fixes. Releases are on Creative Downloads (see [README.md](README.md#download)); a fix ships as a new release, not as a patch to an older one.

## Reporting a vulnerability

Report it privately through GitHub: open <https://github.com/Crasius/code-view/security/advisories/new> (the repository's **Security** tab, **Report a vulnerability**). This opens a private advisory that only you and the maintainers see. Please do not open a public issue, pull request or discussion about it.

A useful report contains:

- the version of Code View (the release you downloaded, or the commit you built) and your system;
- what an attacker can do, and what they need for it (a crafted pull request, a link, local access);
- the steps to reproduce it, as small as you can make them, and what you expected instead;
- whether a host's access token, your files or another account is affected.

Never put a real access token, password or private repository content into the report. If a token was exposed, revoke it at its host first.

## What you can expect

- An answer in the advisory once a maintainer has read the report, and an assessment of whether it is a vulnerability.
- Questions there while it is being fixed, and a note when the fix is released.
- Credit in the published advisory if you want it.

Code View is maintained by a small team, so there is no fixed response time. Please give us a reasonable time to release a fix before you publish details.

What the app touches on your machine and network is listed in [README.md](README.md#what-code-view-does-on-your-machine).
