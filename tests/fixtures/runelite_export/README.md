# RuneLite export fixtures

| File | Provenance |
| --- | --- |
| `character.synthetic.json` | **Synthetic.** Hand-written to follow the `character.json` shape that `CharacterStateExporterPlugin.java` at upstream commit `d790a93` writes (see `docs/research/runelite-export-formats.md`). Every value is invented: the account name, session id and world are placeholders, and the levels and experience are arbitrary but consistent with the standard experience table. It is not derived from any real account. |

No authentic export, personal identifier, account name or path may be committed here. Authentic exports are only inspected read-only on the maintainer's machine, and only their structure and types are recorded.
