# Project Murmur

Project Murmur is an ASCII life simulation. This repository currently contains
only its development foundation: a reusable PASM architecture model, a small
initial specification, and project conventions. No gameplay systems are
implemented yet.

## Architecture model

PASM (Project Architecture & System Model) keeps the planned system boundaries,
authority, and implementation evidence explicit as the project grows.

```powershell
uv sync --group dev
uv run pasm validate
uv run pasm scan --json
```

The starting model is intentionally minimal and gameplay-neutral. Add the
first gameplay model before implementing its corresponding system.
