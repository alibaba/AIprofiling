// SPDX-License-Identifier: Apache-2.0
// Synthesizer — takes tool evidence + plan, calls LLM to produce Markdown conclusion.
'use strict';

const { SYSTEM_PROMPT, SYNTHESIZER_PROMPT, REFINE_PROMPT } = require('./prompts');

function buildEvidenceBlock(results) {
    const sections = [];
    for (const r of results) {
        if (!r.ok) continue;
        let evidenceStr = JSON.stringify(r.evidence, null, 2);
        if (evidenceStr.length > 12000) {
            evidenceStr = evidenceStr.slice(0, 12000) + '\n... (truncated)';
        }
        sections.push(`### tool: ${r.name}\nsummary: ${r.summary}\n\`\`\`json\n${evidenceStr}\n\`\`\``);
    }
    return sections.join('\n\n');
}

async function synthesize({ question, plan, dataProfile, results, llm, rounds = 1, onRound }) {
    const q = (question || '').trim() || '请分析性能瓶颈并给出优化建议。';
    const evidenceBlock = buildEvidenceBlock(results);

    const userPrompt = `问题：${q}
识别的意图：${plan.intents.join(', ')}${plan.focus ? `（focus：${plan.focus}）` : ''}

数据概览：
事件数=${dataProfile?.evidence?.eventCount || '?'}, 时间跨度=${dataProfile?.evidence?.timeSpanMs || '?'}ms, GPU设备=${JSON.stringify(dataProfile?.evidence?.devices || [])}

证据（按工具聚合）：
${evidenceBlock}`;

    const notify = onRound || (() => {});
    const totalRounds = Math.max(1, Math.min(5, rounds | 0));

    // Round 1: initial synthesis.
    let { text } = await llm.complete({
        system: `${SYSTEM_PROMPT}\n\n${SYNTHESIZER_PROMPT}`,
        user: userPrompt,
        maxTokens: 8000,
        timeoutMs: 180000,
    });
    notify({ round: 1, total: totalRounds, chars: text.length });

    // Rounds 2..N: self-refine against the same evidence (no new tools/data).
    for (let round = 2; round <= totalRounds; round += 1) {
        const refineSystem = `${SYSTEM_PROMPT}\n\n${REFINE_PROMPT
            .replace('{round}', String(round))
            .replace('{total}', String(totalRounds))}`;
        const refineUser = `${userPrompt}

上一轮报告：
${text}`;
        try {
            const r = await llm.complete({
                system: refineSystem,
                user: refineUser,
                maxTokens: 8000,
                timeoutMs: 180000,
            });
            if (r.text && r.text.trim()) text = r.text;
            notify({ round, total: totalRounds, chars: text.length });
        } catch (err) {
            // A failed refine round keeps the last good report rather than aborting.
            notify({ round, total: totalRounds, chars: text.length, error: err.message });
            break;
        }
    }

    return text;
}

module.exports = { synthesize };
