// SPDX-License-Identifier: Apache-2.0
// Planner — intent classification via LLM with keyword fallback.
'use strict';

const { PLANNER_PROMPT } = require('./prompts');

const KEYWORD_RULES = [
    { pattern: /周期|突增|抖动|periodic|burst|spike|jitter/i, intent: 'periodicity' },
    { pattern: /OOM|显存|内存|memory|alloc|泄漏|leak/i, intent: 'memory' },
    { pattern: /kernel|热点|hot|瓶颈|bottleneck|top|耗时/i, intent: 'hotspot' },
    { pattern: /idle|空闲|空隙|gap|bubble|利用率|util/i, intent: 'idle' },
    { pattern: /launch|延迟|latency|overhead|启动/i, intent: 'launch' },
    { pattern: /概览|总览|overview|summary|全面/i, intent: 'overview' },
];

function keywordFallback(question) {
    const intents = new Set();
    for (const rule of KEYWORD_RULES) {
        if (rule.pattern.test(question)) intents.add(rule.intent);
    }
    if (intents.size === 0) intents.add('overview').add('hotspot');
    return Array.from(intents);
}

async function planIntents({ question, dataProfile, llm }) {
    const q = (question || '').trim() || '请分析性能瓶颈并给出优化建议。';
    const fallbackIntents = keywordFallback(q);

    // Build compact data overview for the planner
    const overview = dataProfile?.evidence
        ? `事件数=${dataProfile.evidence.eventCount}, 时间跨度=${dataProfile.evidence.timeSpanMs}ms, ` +
          `categories: ${Object.keys(dataProfile.evidence.catBreakdown || {}).join(', ')}, ` +
          `hasKernel=${dataProfile.evidence.hasKernel}, hasMemory=${dataProfile.evidence.hasMemory}`
        : '';

    const userPrompt = `问题：${q}\n数据概览：${overview}`;

    try {
        const { text } = await llm.complete({
            system: PLANNER_PROMPT,
            user: userPrompt,
            schema: {
                type: 'object',
                properties: {
                    intents: { type: 'array', items: { type: 'string' } },
                    focus: { type: 'string' },
                    rationale: { type: 'string' },
                },
                required: ['intents'],
            },
            maxTokens: 200,
            timeoutMs: 30000,
        });

        // Parse JSON from LLM — strip markdown fences if present
        const cleaned = text.replace(/^```(?:json)?\s*/m, '').replace(/```\s*$/m, '').trim();
        const parsed = JSON.parse(cleaned);
        const intents = Array.isArray(parsed.intents) && parsed.intents.length > 0
            ? parsed.intents.filter((i) => typeof i === 'string')
            : fallbackIntents;
        return {
            intents,
            focus: parsed.focus || '',
            rationale: parsed.rationale || '',
            source: 'llm',
        };
    } catch (err) {
        // LLM failed — use keyword fallback silently
        return {
            intents: fallbackIntents,
            focus: '',
            rationale: `LLM planner failed (${err.message?.slice(0, 80)}), using keyword rules`,
            source: 'fallback',
        };
    }
}

module.exports = { planIntents, keywordFallback };
