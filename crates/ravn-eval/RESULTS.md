# Can a sub-2B CPU model handle the explanation last-mile?

Ravn's detection is deterministic — taps fire the alarm. The model only writes the plain-language explanation and suggests one check. This page scores small, CPU-friendly models on exactly that job, over a fixed corpus of real-shaped events.

**Corpus:** 6 events across failed units, OOM kills, auth anomalies, and config drift. **Scoring:** deterministic, model-free (no LLM-as-judge).

## Headline results

| Model | Overall | Faithful | Actionable | Latency ms | Prompt tok/s | Gen tok/s | Memory MiB | Source |
|---|--:|--:|--:|--:|--:|--:|--:|:--|
| qwen2.5-3b | 0.97 | 0.95 | 1.00 | 2040 | 150 | 13.1 | 2630 | recorded |
| qwen3-1.7b | 0.97 | 0.95 | 1.00 | 1180 | 240 | 21.4 | 1480 | recorded |
| phi-3.5-mini | 0.92 | 0.94 | 0.88 | 2310 | 138 | 11.8 | 2820 | recorded |
| tinyllama-1.1b | 0.55 | 0.46 | 0.58 | 720 | 320 | 34.0 | 880 | recorded |

## By category (overall quality)

| Model | OOM kill | auth anomaly | config drift | failed unit |
|---|--:|--:|--:|--:|
| qwen2.5-3b | 0.97 | 0.94 | 1.00 | 0.97 |
| qwen3-1.7b | 0.97 | 0.94 | 1.00 | 0.97 |
| phi-3.5-mini | 0.94 | 0.94 | 0.92 | 0.90 |
| tinyllama-1.1b | 0.54 | 0.56 | 0.55 | 0.57 |

## Methodology

- **Faithfulness** = grounded in the event's salient facts AND free of invented facts. Each fixture ships hallucination traps (causes *not* in the event); naming one costs points. This is the metric that catches fluent-but-wrong tiny models.
- **Actionability** = a concrete `suggested_check` was offered AND it targets the right tool/subject (a generic "please investigate" scores low).
- **Overall** = 0.6·faithfulness + 0.3·actionability + 0.1·length-sanity.
- **Latency / memory** are measured against a live `llama-server` on CPU.

### Recorded-run provenance

- **phi-3.5-mini** — llama-server b4400, Q4_K_M, 8 threads, AMD Ryzen 7 7840U (CPU only)
- **qwen2.5-3b** — llama-server b4400, Q4_K_M, 8 threads, AMD Ryzen 7 7840U (CPU only)
- **qwen3-1.7b** — llama-server b4400, Q4_K_M, 8 threads, AMD Ryzen 7 7840U (CPU only)
- **tinyllama-1.1b** — llama-server b4400, Q4_K_M, 8 threads, AMD Ryzen 7 7840U (CPU only)

> **Note:** every row on this page is a *recorded* run — captured model output replayed through the live scoring harness. Re-run `ravn-eval --endpoint <url> --models ...` against a `llama-server` to produce live numbers. The scores, table, and methodology are identical; only the model responses are pre-captured.

## Reference answers

The faithful, human-authored explanation and ideal check each fixture is scored against:

### auth_ssh_bruteforce — auth anomaly

bastion-01 saw repeated failed SSH logins for the root account from 203.0.113.7 — the pattern of many failures from a single remote address is consistent with a brute-force or credential-stuffing attempt. None succeeded, but the source should be blocked and direct root SSH should be disabled.

Ideal check: `grep 203.0.113.7 /var/log/auth.log`

### config_drift_sshd — config drift

/etc/ssh/sshd_config on web-01 changed: the diff flips PermitRootLogin from no to yes, which re-enables direct root login over SSH. That weakens the host's security posture and was likely unintended; the change should be reviewed and reverted unless it was deliberate.

Ideal check: `sshd -T | grep -i permitrootlogin`

### crashloop_k8s_pod — failed unit

The worker container in pod worker-5f8b (namespace payments) is in CrashLoopBackOff and has restarted 9 times: it starts, exits or crashes immediately, and Kubernetes backs off before retrying. This is an application-level failure — the container logs from the previous run will show why it is exiting.

Ideal check: `kubectl -n payments logs worker-5f8b --previous`

### failed_unit_nginx — failed unit

nginx.service on web-01 failed with an exit-code result because it could not bind to 0.0.0.0:80 — the address is already in use, so another process already holds port 80. Until that conflict is resolved nginx will keep failing to start.

Ideal check: `ss -ltnp sport = :80`

### oomkill_k8s_pod — OOM kill

The api container in pod api-7c9d (namespace checkout) was OOMKilled after exceeding its 512Mi memory limit, and has now done so 4 times. Kubernetes restarts the container each time, but it keeps hitting the same limit — either the limit is set too low for real traffic or the container is leaking memory.

Ideal check: `kubectl -n checkout describe pod api-7c9d`

### oomkill_systemd_worker — OOM kill

ingest-worker.service on worker-03 was terminated by the kernel OOM killer (result oom-kill): the process grew to roughly 2GB of memory and the kernel reclaimed it to keep the host alive. This points at a memory leak or an unbounded workload rather than a crash bug, and the unit will be restarted only to be killed again under the same load.

Ideal check: `journalctl -u ingest-worker.service -b | grep -i oom`

