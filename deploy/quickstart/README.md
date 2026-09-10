# AIProf quickstart

Boot the full stack on one host in one command:

```bash
cd deploy/docker && docker compose up --build
```

Services that come up:

| service            | port  | purpose                                    |
|--------------------|-------|--------------------------------------------|
| `aiprof-ui`        | 8080  | Web UI (open http://localhost:8080)        |
| `aiprof-collector` | 7101  | Session upload endpoint                    |
| `aiprof-query`     | 7102  | List/read sessions + folded stacks         |
| `aiprof-report`    | 7103  | AI analysis report cache                   |
| `aiprof-ai`        | 8642  | OpenAI-compatible gateway (offline default)|
| `aiprof-agent`     | –     | Loops: collect mock session, upload        |

Send one session by hand (works even without the agent container):

```bash
python3 agent-py/aiprof_agent.py --mock --once \
  --endpoint http://localhost:7101/api/v1/collector/upload
```

Then open http://localhost:8080/#/sessions and pick the new session.

## Tearing down

```bash
docker compose -f deploy/docker/docker-compose.yml down -v
```

The `-v` also drops the `aiprof-data` volume.
