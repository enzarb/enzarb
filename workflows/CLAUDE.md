# enzarb-workflows

One of several separate enzarb repositories. The top-level `enzarb` repo (github.com/enzarb/enzarb, checked out one directory up, when checked out with the others) holds the architecture docs and cross-repo conventions: see `../CLAUDE.md` and `../docs/architecture.md`.

Work inside this repo with its own build/test commands. Changes to shared contracts (enzarb-proto frames, `workflows_api`/`app_api` schemas) must stay backward compatible.
