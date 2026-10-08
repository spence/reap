# DEC-REAP-NATIVE-IDENTITY — preserve macOS identity across installs

Status: accepted engineering decision, 2026-10-07.

The owner requested native micro.dev signing on both Macs so approved Reap
installs remain the same application across rebuilds, without permission hacks.

The macOS installer uses an optional machine-local `.codesign.env` containing
only the public `REAP_CODESIGN_IDENTITY` certificate identifier. A nonempty
explicit environment value takes precedence. Signing uses the fixed code
identifier `dev.micro.reap` and Apple's synthesized designated requirement.

Build, sign, and verify complete in staging before the installed executable
is replaced. This keeps failed signing and the unsigned build window from
changing the installed identity. No configured signer may downgrade an
already certificate-signed installation to ad-hoc code.

Native Keychain authorization and application privacy grants remain macOS/user
decisions. Reap does not reset TCC, export keys, weaken Keychain access policy,
or promise that signing removes every possible notification.
