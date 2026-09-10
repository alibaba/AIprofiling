#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// CLI entry for local-profiling-agent.
// Usage:
//   node bin/cli.js --trace /path/trace.json --question "..." [--out /dir] [--dry-run]
//
// Env for LLM (when not --dry-run):
//   LLM_BASE_URL, LLM_API_KEY, LLM_MODEL
'use strict';

const path = require('path');
const fs = require('fs');
const minimist = require('minimist');

const argv = minimist(process.argv.slice(2), {
    string: ['trace', 'question', 'out', 'llm-base-url', 'api-key', 'model'],
    boolean: ['dry-run', 'help'],
    alias: { t: 'trace', q: 'question', o: 'out', h: 'help' },
});

if (argv.help || !argv.trace) {
    console.log(`local-profiling-agent CLI

Usage:
  node bin/cli.js --trace <path> [--question "..."] [--out <dir>] [--dry-run]

Options:
  --trace, -t       Path to chrome-tracing JSON file (required)
  --question, -q    Analysis question (default: general overview)
  --out, -o         Output directory (default: ./agent-run-<ts>)
  --dry-run         Skip LLM calls, only run ingest + tools → evidence/*.json
  --llm-base-url    LLM endpoint (or env LLM_BASE_URL)
  --api-key         LLM API key (or env LLM_API_KEY)
  --model           LLM model name (or env LLM_MODEL, default qwen-plus)
  --help, -h        Show this message
`);
    process.exit(argv.help ? 0 : 1);
}

(async () => {
    const t0 = Date.now();
    const { ingestTrace } = require('../src/traceIngest');
    const { getDataProfile } = require('../src/tools/getDataProfile');
    const { analyzeStatistics } = require('../src/tools/analyzeStatistics');
    const { analyzeKernel } = require('../src/tools/analyzeKernel');
    const { analyzeGpuIdle } = require('../src/tools/analyzeGpuIdle');
    const { analyzePeriodicity } = require('../src/tools/analyzePeriodicity');
    const { analyzeMemory } = require('../src/tools/analyzeMemory');
    const { analyzeLaunchOverhead } = require('../src/tools/analyzeLaunchOverhead');

    const tracePath = path.resolve(argv.trace);
    const outDir = argv.out ? path.resolve(argv.out) : path.join(process.cwd(), `agent-run-${Date.now()}`);
    fs.mkdirSync(path.join(outDir, 'evidence'), { recursive: true });

    console.log(`[ingest] ${tracePath}`);
    const store = await ingestTrace(tracePath, {
        onProgress: ({ pct, eventCount }) => {
            if (pct % 25 === 0) process.stdout.write(`  ${pct}% (${eventCount} events)\r`);
        },
    });
    console.log(`[ingest] done: ${store.meta.eventCount} events in ${store.meta.ingestMs}ms`);
    fs.writeFileSync(path.join(outDir, 'trace-meta.json'), JSON.stringify(store.meta, null, 2));

    // Run all tools
    console.log('[tools] running all 7 tools...');
    const toolResults = [
        getDataProfile(store),
        analyzeStatistics(store),
        analyzeKernel(store),
        analyzeGpuIdle(store),
        analyzePeriodicity(store),
        analyzeMemory(store),
        analyzeLaunchOverhead(store),
    ];
    for (const r of toolResults) {
        const fname = `${r.name}.json`;
        fs.writeFileSync(path.join(outDir, 'evidence', fname), JSON.stringify(r, null, 2));
        const status = r.ok ? '✓' : '✗';
        console.log(`  ${status} ${r.name}: ${r.summary}`);
    }

    if (argv['dry-run']) {
        const elapsed = ((Date.now() - t0) / 1000).toFixed(1);
        console.log(`\n[dry-run] done in ${elapsed}s. Evidence written to ${outDir}/evidence/`);
        process.exit(0);
    }

    // Full mode — needs LLM
    const baseUrl = argv['llm-base-url'] || process.env.LLM_BASE_URL;
    const apiKey = argv['api-key'] || process.env.LLM_API_KEY;
    const model = argv.model || process.env.LLM_MODEL || 'qwen-plus';
    if (!baseUrl || !apiKey) {
        console.error('\n[error] LLM credentials required (--llm-base-url + --api-key, or env LLM_BASE_URL + LLM_API_KEY)');
        console.error('        Use --dry-run to skip LLM and only produce evidence files.');
        process.exit(1);
    }

    const { createOpenAICompatibleBackend } = require('../src/llm/openaiCompatible');
    const { planIntents } = require('../src/planner');
    const { synthesize } = require('../src/synthesizer');

    const llm = createOpenAICompatibleBackend({ baseUrl, apiKey, model });
    const question = argv.question || '请分析性能瓶颈并给出优化建议。';

    console.log(`[plan] question: "${question}"`);
    const dataProfile = toolResults[0]; // getDataProfile
    const plan = await planIntents({ question, dataProfile, llm });
    console.log(`[plan] intents: ${plan.intents.join(', ')} | focus: ${plan.focus}`);
    fs.writeFileSync(path.join(outDir, 'plan.json'), JSON.stringify(plan, null, 2));

    console.log('[synth] generating conclusion...');
    const markdown = await synthesize({ question, plan, dataProfile, results: toolResults, llm });
    fs.writeFileSync(path.join(outDir, 'conclusion.md'), markdown, 'utf-8');
    console.log(`[synth] done. conclusion.md written (${markdown.length} chars)`);

    const metrics = {
        ingestMs: store.meta.ingestMs,
        toolCount: toolResults.length,
        totalMs: Date.now() - t0,
        eventCount: store.meta.eventCount,
        model,
    };
    fs.writeFileSync(path.join(outDir, 'metrics.json'), JSON.stringify(metrics, null, 2));
    console.log(`\n[done] total ${(metrics.totalMs / 1000).toFixed(1)}s. Output: ${outDir}`);
})().catch((err) => {
    console.error('[fatal]', err.message || err);
    process.exit(1);
});
