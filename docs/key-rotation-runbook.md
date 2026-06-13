---
layout: page
title: Command-Signing Key Rotation & Compromise Response
permalink: /key-rotation-runbook/
---

# Command-Signing Key Rotation & Compromise Response

This runbook covers the Ed25519 key that the control plane uses to sign every
remediation `CommandEnvelope`. Agents and the privileged actuator verify that
signature before any self-healing action runs, so this key is the trust anchor
for the whole remediation path.

As of #150 the key is **versioned** (each key has a `kid`) and verifiers trust a
**keyring** — a set of public keys — rather than a single pinned key. That lets
you rotate the signing key with **zero command-verification failures and no fleet
re-enrollment**: publish the new key alongside the old, let agents pin it on their
next check-in, then retire the old key after an overlap window.

> Founding doctrine: signatures stay **mandatory** at every hop. Nothing in this
> runbook weakens verification — rotation only changes *which* keys are trusted,
> never *whether* a signature is required.

---

## How it works (the moving parts)

| Piece | Where | Role |
| --- | --- | --- |
| `kid` | `CommandEnvelope.kid` (ravn-core) | First 16 base64 chars of the signing key's public half. Stamped at signing, covered by the signature. |
| Active signing key | control plane, `RAVN_COMMAND_KEY` | Signs new commands; advertised as the keyring's default. |
| Previous keys | control plane, `RAVN_COMMAND_PREVIOUS_KEYS_DIR` | Public keys still trusted during the overlap window. |
| Keyring document | `GET /command-keys` and `EnrollResponse.command_keyring` | `{ "active_kid": "...", "keys": { kid: pubkey_b64, ... } }`. |
| Pinned keyring | agent, `<cred_dir>/command_keyring.json` | What the agent verifies against; refreshed every poll. |
| Actuator trust | actuator, `RAVN_COMMAND_KEYRING` (path) | Independently re-verifies; reloads the same file the agent pins. |

**Verification rule.** A verifier selects the key by the envelope's `kid`. If the
`kid` is unknown (forged or retired) the command is rejected (`UnknownKey`). A
legacy envelope with **no** `kid` (minted before #150) verifies against the
keyring's default key, so in-flight pre-rotation commands keep working.

**Fetch-on-check-in.** On every command poll the agent first fetches
`GET /command-keys` over its authenticated channel, and if the trust set changed
it atomically re-pins `command_keyring.json` and swaps it in *before* processing
commands. A new key is therefore trusted on the fleet before any command signed
with it arrives. A fetch/parse failure is non-fatal: the agent keeps its last good
keyring (fail safe, never fail open).

---

## Symptoms

Use this runbook when you see any of:

- **Routine rotation due.** Key age exceeds policy (e.g. 90 days), or an operator
  with key access has off-boarded.
- **Suspected compromise.** The signing key file (`RAVN_COMMAND_KEY`) may have
  leaked: host breach on the control plane, key in a backup/snapshot that left the
  trust boundary, or a secret accidentally committed/logged.
- **Verification failures after a change.** Agents logging
  `command signed by an untrusted or retired key` (`UnknownKey`) or
  `command signature does not verify` — usually a botched rotation (old key
  retired too early, or the keyring not published).

---

## Diagnostics (safe, read-only)

```bash
# 1. What keyring is the control plane currently advertising?
curl -fsS -H "Authorization: Bearer $RAVN_ADMIN_TOKEN" \
  "$CONTROL_PLANE/command-keys" | jq .
# → { "active_kid": "<kid>", "keys": { "<kid>": "<pubkey_b64>", ... } }

# 2. What keyring has an agent pinned? (on the host)
sudo cat "$RAVN_CRED_DIR/command_keyring.json" | jq .

# 3. Derive the kid of a key file you hold (active or a previous pubkey):
#    kid = first 16 chars of base64(public key). For a *public* key file:
head -c 64 "$PUBKEY_FILE" | cut -c1-16

# 4. Recent verification rejections on a host:
journalctl -u ravnd | grep -E "untrusted or retired key|signature does not verify"
```

A healthy mid-rotation state: `GET /command-keys` lists **two** keys, `active_kid`
points at the **new** one, and agents' pinned `command_keyring.json` matches.

---

## Remediation — routine rotation (zero downtime)

Run these in order. The overlap window must be longer than one agent poll interval
(`RAVN_COMMAND_POLL_SECS`, default 10s) plus the longest in-flight command TTL
(`RAVN_COMMAND_TTL_SECS`, default 300s). A window of **24h** is a safe default.

1. **Generate the new signing key** (base64 Ed25519 private key, mode `0600`).
   You can let the control plane generate it by pointing `RAVN_COMMAND_KEY` at a
   fresh path on next start, or pre-generate one out of band.

2. **Keep the old key trusted.** Copy the **public** half of the *current* key
   into the previous-keys directory:

   ```bash
   mkdir -p "$RAVN_COMMAND_PREVIOUS_KEYS_DIR"
   # Public key = base64 of the 32-byte verifying key. Extract from the current
   # control plane (it is the active_kid's value in GET /command-keys), e.g.:
   curl -fsS -H "Authorization: Bearer $RAVN_ADMIN_TOKEN" "$CONTROL_PLANE/command-keys" \
     | jq -r '.keys[.active_kid]' > "$RAVN_COMMAND_PREVIOUS_KEYS_DIR/old-$(date +%F).b64"
   ```

3. **Cut over the active key.** Point `RAVN_COMMAND_KEY` at the new key file and
   restart the control plane. It now signs with the new `kid` and advertises a
   keyring containing **both** keys (new = active/default, old = still trusted).

4. **Let the fleet converge.** Agents pick up the new keyring on their next poll
   and re-pin it; the actuator reloads it on its next command. During this window,
   commands signed by **either** key verify — no failures, no re-enrollment.

   Verify convergence: spot-check a few hosts' `command_keyring.json` against
   `GET /command-keys` (both should list two keys with the new `active_kid`).

5. **Retire the old key** after the overlap window: remove its file from
   `RAVN_COMMAND_PREVIOUS_KEYS_DIR` and restart the control plane. The published
   keyring now holds only the new key; agents drop the old key on their next poll.
   Any command still signed by the old key (there should be none) is now rejected.

**Rollback.** If step 3/5 misbehaves, the old key is still in the keyring during
the window, so reverting `RAVN_COMMAND_KEY` to the old key (and removing the new
one from previous-keys) restores the prior state with no agent action required.

---

## Remediation — suspected or confirmed compromise (emergency)

A compromise means the **old key must be distrusted as fast as the fleet can
converge** — you cannot afford a long overlap, because the attacker can forge
valid commands until the old key is retired everywhere.

1. **Contain first.** Treat the control-plane host as compromised: rotate its
   other secrets (`RAVN_ADMIN_TOKEN`, enrollment bootstrap token, mTLS CA key if
   exposed) per their own runbooks. Revoke access for any party who may hold the
   leaked signing key.

2. **Generate a new signing key on a clean host** and deploy it as
   `RAVN_COMMAND_KEY`.

3. **Do _not_ add the compromised key to `RAVN_COMMAND_PREVIOUS_KEYS_DIR`.**
   Skipping the overlap means the published keyring contains **only** the new
   key, so the compromised key is `UnknownKey` everywhere the moment an agent
   re-polls. The trade-off: any *legitimate* in-flight command signed by the old
   key is also rejected — acceptable under compromise (it will simply be
   re-proposed and re-signed with the new key).

4. **Restart the control plane** and force fast convergence:
   - Temporarily lower `RAVN_COMMAND_POLL_SECS` if you need sub-10s fleet-wide
     pickup, or
   - Restart `ravnd` on critical hosts so they re-fetch `command-keys` immediately.

5. **Confirm the compromised `kid` is gone** from `GET /command-keys` and from a
   sample of hosts' pinned `command_keyring.json`. Grep for the old `kid` in
   `command signed by an untrusted or retired key` rejections to confirm any
   replayed old-key commands are being refused.

6. **Audit.** Review the command/result ledger and `RemediationRecord` history for
   the compromise window for any command you did not authorise. The actuator's
   independent re-verification and the at-most-once ledger bound the blast radius,
   but assume the attacker could sign anything the old key could until retirement.

7. **Post-incident.** If the compromise reached an agent's pinned keyring or the
   actuator, also rotate the agent's mTLS identity (re-enroll) — but note key
   rotation itself never requires re-enrollment.

---

## Verification (done when)

- `GET /command-keys` shows the intended trust set with the correct `active_kid`.
- A new remediation signs with the new `kid` (check a fresh `RemediationRecord`).
- No `untrusted or retired key` / `signature does not verify` rejections on a
  representative sample of hosts after convergence.
- For routine rotation: at no point during the window did any host log a
  verification failure (zero-downtime goal met).
- For compromise: the old `kid` is absent from every sampled host and is actively
  rejected if presented.

---

## Reference — configuration

| Variable | Component | Meaning |
| --- | --- | --- |
| `RAVN_COMMAND_KEY` | control plane | Path to the **active** Ed25519 signing key (base64, `0600`). |
| `RAVN_COMMAND_PREVIOUS_KEYS_DIR` | control plane | Directory of **previous public** keys trusted during the overlap window (one base64 pubkey per file). |
| `RAVN_COMMAND_TTL_SECS` | control plane | Signed-command validity window (default 300s). Sets the minimum safe overlap. |
| `RAVN_COMMAND_POLL_SECS` | agent | How often the agent re-fetches the keyring and commands (default 10s). |
| `RAVN_COMMAND_KEYRING` | actuator | Path to the pinned keyring JSON (usually the agent's `command_keyring.json`); reloaded per command. |
| `RAVN_COMMAND_PUBKEY` | actuator | Single-key fallback when no keyring file is present (backward compatibility). |

See also: [Architecture](./architecture.md), [Runbook Authoring Guide](./runbook-authoring-guide.md).
