// SPDX-License-Identifier: Apache-2.0
// Main entry — orchestrates ingest → tools → planner → synthesizer.
'use strict';

const { ingestTrace } = require('./traceIngest');
const { getDataProfile } = require('./tools/getDataProfile');
const { analyzeStatistics } = require('./tools/analyzeStatistics');
const { analyzeKernel } = require('./tools/analyzeKernel');
const { analyzeGpuIdle } = require('./tools/analyzeGpuIdle');
const { analyzePeriodicity } = require('./tools/analyzePeriodicity');
const { analyzeMemory } = require('./tools/analyzeMemory');
const { analyzeLaunchOverhead } = require('./tools/analyzeLaunchOverhead');
const { planIntents } = require('./planner');
const { synthesize } = require('./synthesizer');
const { createOpenAICompatibleBackend } = require('./llm/openaiCompatible');

const ALL_TOOLS = {
    getDataProfile,
    analyzeStatistics,
    analyzeKernel,
    analyzeGpuIdle,
    analyzePeriodicity,
    analyzeMemory,
    analyzeLaunchOverhead,
};

const INTENT_TO_TOOLS = {
    overview: ['analyzeStatistics'],
    hotspot: ['analyzeKernel'],
    idle: ['analyzeGpuIdle'],
    periodicity: ['analyzePeriodicity', 'analyzeKernel'],
    memory: ['analyzeMemory'],
    launch: ['analyzeLaunchOverhead', 'analyzeKernel'],
};

function mapIntentsToTools(intents) {
    const names = new Set(['getDataProfile']); // always run
    for (const intent of intents) {
        const tools = INTENT_TO_TOOLS[intent] || [];
        for (const t of tools) names.add(t);
    }
    return Array.from(names);
}

async function runLocalProfilingAgent({ traceFilePath, question, llm, options = {} }) {
    const {
        maxEvents = 5_000_000,
        maxTools = 7,
        quickScan = true,
        rounds = 2,
        onProgress,
    } = options;

    const progress = onProgress || (() => {});
    const t0 = Date.now();
    const warnings = [];

    // 1. Ingest
    progress({ phase: 'ingest', msg: 'parsing trace file...' });
    const store = await ingestTrace(traceFilePath, { maxEvents, onProgress: (p) => progress({ phase: 'ingest', ...p }) });
    const ingestMs = store.meta.ingestMs;
    progress({ phase: 'ingest', msg: `done: ${store.meta.eventCount} events in ${ingestMs}ms` });

    // 2. Data profile (always first)
    const dataProfile = getDataProfile(store);

    // 3. Plan intents
    progress({ phase: 'plan', msg: 'classifying intent...' });
    const planT0 = Date.now();
    const plan = await planIntents({ question, dataProfile, llm });
    const planMs = Date.now() - planT0;
    progress({ phase: 'plan', msg: `intents: ${plan.intents.join(', ')}` });

    // 4. Run tools
    progress({ phase: 'tool', msg: 'running analysis tools...' });
    const toolT0 = Date.now();
    let toolNames = mapIntentsToTools(plan.intents);
    if (quickScan) {
        // Ensure core tools always run
        for (const t of ['analyzeStatistics', 'analyzeKernel', 'analyzeGpuIdle']) {
            if (!toolNames.includes(t)) toolNames.push(t);
        }
    }
    toolNames = toolNames.slice(0, maxTools);

    const toolResults = [dataProfile];
    for (const name of toolNames) {
        if (name === 'getDataProfile') continue; // already ran
        const fn = ALL_TOOLS[name];
        if (!fn) { warnings.push(`unknown tool: ${name}`); continue; }
        try {
            const r = fn(store);
            toolResults.push(r);
        } catch (err) {
            warnings.push(`${name} failed: ${err.message}`);
            toolResults.push({ name, ok: false, summary: err.message, evidence: {} });
        }
    }
    const toolMs = Date.now() - toolT0;
    progress({ phase: 'tool', msg: `done: ${toolResults.length} tools in ${toolMs}ms` });

    // 5. Synthesize
    progress({ phase: 'synth', msg: 'generating conclusion...' });
    const synthT0 = Date.now();
    const markdown = await synthesize({
        question, plan, dataProfile, results: toolResults, llm, rounds,
        onRound: (r) => progress({ phase: 'synth', msg: `refine round ${r.round}/${r.total}: ${r.chars} chars${r.error ? ` (stopped: ${r.error})` : ''}` }),
    });
    const synthMs = Date.now() - synthT0;
    progress({ phase: 'synth', msg: `done: ${markdown.length} chars in ${synthMs}ms` });

    return {
        markdown,
        intents: plan.intents,
        toolResults,
        dataProfile,
        plan,
        metrics: {
            ingestMs,
            planMs,
            toolMs,
            synthMs,
            totalMs: Date.now() - t0,
            eventCount: store.meta.eventCount,
        },
        warnings,
    };
}

module.exports = {
    runLocalProfilingAgent,
    createOpenAICompatibleBackend,
    // Expose for direct use / testing
    ingestTrace,
    ALL_TOOLS,
    planIntents,
    synthesize,
};
