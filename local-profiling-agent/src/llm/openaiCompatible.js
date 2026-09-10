// SPDX-License-Identifier: Apache-2.0
// OpenAI-compatible LLM backend. Works with DashScope compatible-mode, vLLM,
// OpenAI, Ollama, and anything that speaks POST /chat/completions.
'use strict';

function createOpenAICompatibleBackend({ baseUrl, apiKey, model }) {
    if (!baseUrl) throw new Error('LLM baseUrl is required');
    if (!apiKey) throw new Error('LLM apiKey is required');
    const resolvedModel = model || 'qwen-plus';
    const url = baseUrl.replace(/\/+$/, '') + '/chat/completions';

    return {
        name: `openai-compatible(${resolvedModel})`,

        async complete({ system, user, schema, maxTokens, timeoutMs }) {
            const messages = [];
            if (system) messages.push({ role: 'system', content: system });
            let userContent = user;
            if (schema) {
                userContent += '\n\n请只输出严格 JSON（不含 markdown 代码块标记 ```），schema: ' + JSON.stringify(schema);
            }
            messages.push({ role: 'user', content: userContent });

            const body = {
                model: resolvedModel,
                messages,
                stream: false,
            };
            if (maxTokens) body.max_tokens = maxTokens;

            const controller = new AbortController();
            const timeout = timeoutMs || 120000;
            const timer = setTimeout(() => controller.abort(), timeout);

            try {
                const resp = await fetch(url, {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json',
                        'Authorization': `Bearer ${apiKey}`,
                    },
                    body: JSON.stringify(body),
                    signal: controller.signal,
                });
                const text = await resp.text();
                if (!resp.ok) throw new Error(`LLM HTTP ${resp.status}: ${text.slice(0, 300)}`);
                let json;
                try { json = JSON.parse(text); } catch { return { text }; }
                const content = json.choices?.[0]?.message?.content || json.output_text || text;
                const usage = json.usage || null;
                return { text: content, usage };
            } finally {
                clearTimeout(timer);
            }
        },
    };
}

module.exports = { createOpenAICompatibleBackend };
