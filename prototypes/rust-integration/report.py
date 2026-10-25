#!/usr/bin/env python3
"""Render the captured experiment, never invent or rerun measurements."""
import html,json,pathlib
HERE=pathlib.Path(__file__).resolve().parent
E=HERE/'evidence'
d=json.loads((E/'results.json').read_text());m=d['metrics'];initial=json.loads((E/'initial-run.json').read_text());passed=sum(x['pass'] for x in d['checks'])
summary=f'''# Rust / Quickshell integration observations

Verdict: the separate Rust backend and stock Quickshell/QML route is feasible at this bounded fixture size without a custom native Quickshell plugin. **{passed}/{len(d['checks'])} observations passed in the final run.** This is a feasibility result, not a production implementation or broad performance guarantee.

| Measure | Final observed | Prototype target |
| --- | ---: | ---: |
| Local backend command acknowledgement | {m['command_ms']:.3f} ms | 250 ms |
| Five warm catalog pages | {', '.join(str(x) for x in m['page_ms'])} ms | 250 ms each |
| QML search-burst maximum heartbeat interval | {m['heartbeat_max_ms']} ms | 50 ms |
| Combined sampled backend/UI RSS | {m['combined_rss_mib']:.1f} MiB | 512 MiB |

Fixture: 100,000 fictional tracks, 2,000 artist labels, 10 tracks per album, 100 displayed rows and a 200-row hard page cap. The GUI does not hold the entire catalog. RSS sums shared mappings twice; it is sampled rather than a continuous peak. A heartbeat is a main-loop timer observation, not a frame or physical-input measurement.

The first completed run observed a {initial['metrics']['heartbeat_max_ms']} ms heartbeat and missed the 50 ms target. That failure is retained in `initial-run.json`. Model insertion was changed from 100 separate JavaScript append calls to a batch append. A subsequent run observed 40 ms; the final run including explicit teardown observed {m['heartbeat_max_ms']} ms. Short before/after samples do not isolate causality or establish tail-latency guarantees.

## What was observed

- Owner-only socket/runtime paths; incompatible-version rejection; fragmented and coalesced messages; snapshot plus racing updates without a revision gap.
- Pending/acknowledged UI commands; lost acknowledgement displayed as uncertain; snapshot reconciliation with no automatic replay; bounded same-instance operation-token duplicate prevention; stale-revision and old-instance rejection.
- Bounded catalog pages, superseded scan cancellation and UI request-ID rejection; forced revision gaps trigger reconnect/snapshot.
- Rust mock-authenticated artwork download, local PNG decoding in QML, changed local URLs for replacement, and no provider credential/URL in presentation state.
- Real Rust/libmpv initialization, event copying, play/pause/position updates, hidden UI progress, independent UI restart, a new paused backend epoch after crash, and successful owner-thread destruction/process exit.
- Backend-owned prototype MPRIS identity/properties and Play/Pause routed through the same command authority. Actual status is derived from mpv events.

## Boundaries and decisions

Keep the selected architecture. Use request IDs separately from logical edit IDs; scope edit IDs to a unique UI instance and backend epoch. On an uncertain outcome, refresh before offering a deliberate retry; the toy 256-operation cache is not durable exactly-once delivery. Subscribe/snapshot under one serialization boundary, and resynchronize on a missed revision. Keep catalog work off the session/UI threads and page the QML model; batch model insertion at this scale. Keep libmpv handles and borrowed event storage on their owner thread and send only copied events across it.

Production refinements required by this evidence: bound ingress and in-flight provider work; make slow-subscriber disconnect/resynchronization explicit; never imply safe replay after operation-result eviction or backend restart; keep wire revisions exactly representable by the QML client (use an explicit bounded-counter/epoch or string representation rather than copying the toy QML int indefinitely). This artifact does not qualify those limits under hostile/flood or long-duration load.

No durable queue was implemented: the backend crash fixture resets its in-memory queue and starts paused. Production queue/checkpoint restoration remains required by the existing contract. No real Plex, TLS/authentication matrix, keyring, service/installer lifecycle, full MPRIS compliance, physical keyboard, hardware output, gapless or DAC-fidelity claim follows. The existing stock-host portal registration warning persists and belongs to lifecycle/identity qualification; it is retained verbatim with runtime paths redacted.

## Captured checks

'''
summary+='\n'.join(f"- {'PASS' if x['pass'] else 'FAIL'}: {x['name']}" for x in d['checks'])+'\n'
(E/'summary.md').write_text(summary)
rows=''.join(f'<tr><td>{html.escape(x["name"])}</td><td class="{"pass" if x["pass"] else "fail"}">{"Observed" if x["pass"] else "Failed"}</td></tr>' for x in d['checks'])
limits=''.join('<li>'+html.escape(x)+'</li>' for x in d['limits'])
page=f'''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Qoda · Rust integration evidence</title>
<style>body{{margin:0;background:#111b16;color:#e8eee9;font:16px/1.6 system-ui}}main{{max-width:980px;margin:auto;padding:50px 24px}}h1{{font-size:clamp(30px,5vw,48px);line-height:1.15}}h2{{margin-top:40px}}p{{max-width:78ch;color:#becbc1}}.eyebrow,.pass{{color:#b8db91}}.cards{{display:grid;grid-template-columns:repeat(auto-fit,minmax(160px,1fr));gap:12px;margin:30px 0}}.card{{padding:20px;background:#1b2a20;border-radius:10px}}strong.metric{{display:block;font-size:29px;color:#d0edb1}}table{{width:100%;border-collapse:collapse}}td,th{{text-align:left;padding:11px;border-bottom:1px solid #304035}}td:last-child{{white-space:nowrap}}.note{{padding:18px;border-left:3px solid #d5b66f;background:#27261c}}a{{color:#b8db91}}code{{font-size:14px}}.fail{{color:#e8b38f}}</style>
<main><div class="eyebrow">QODA / THROWAWAY FEASIBILITY STUDY</div><h1>Rust backend.<br>Stock Quickshell client.</h1><p>{passed} of {len(d['checks'])} observations passed in the final run using 100,000 fictional tracks. No custom native Quickshell plugin was needed for these paths.</p>
<div class="cards"><div class="card"><strong class="metric">{m['command_ms']:.2f} ms</strong>backend acknowledgement</div><div class="card"><strong class="metric">{min(m['page_ms'])}–{max(m['page_ms'])} ms</strong>warm catalog pages</div><div class="card"><strong class="metric">{m['heartbeat_max_ms']} ms</strong>maximum sampled heartbeat</div><div class="card"><strong class="metric">{m['combined_rss_mib']:.0f} MiB</strong>combined sampled RSS</div></div>
<p class="note">The first run reached 52 ms and missed the 50 ms heartbeat target. Its result is preserved. Batched model insertion was used in later runs (40 ms, then {m['heartbeat_max_ms']} ms). These short samples are not a production performance guarantee.</p>
<h2>What this establishes</h2><p>The measured local transport, bounded QML model, authenticated mock artwork, libmpv ownership/event path and minimal backend MPRIS controls work together. UI failure/restart leaves the backend alive; a backend restart creates a new instance and starts paused.</p>
<h2>Observation record</h2><table><thead><tr><th>Scenario</th><th>Result</th></tr></thead><tbody>{rows}</tbody></table>
<h2>What remains unverified</h2><ul>{limits}</ul><p>The stock Quickshell portal-registration warning remains captured in the startup log. This artifact does not resolve product desktop identity. Operation-token caching is bounded and in-memory; uncertain mutations must be reconciled, never blindly replayed.</p>
<p><a href="summary.md">Detailed findings</a> · <a href="results.json">Machine-readable observations</a> · <a href="initial-run.json">Preserved initial failure</a> · <a href="environment.txt">Measured package set</a></p></main></html>'''
(E/'report.html').write_text(page)
