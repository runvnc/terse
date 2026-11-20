# Section numbers of earlier drafts

Until November 2026 the machine specification was one file, `SPEC.md`, with
numbered sections, cited as `SPEC 12.2`. It is now split into the core
machine (`SPEC.md`) and one document per further device (`spec/`), and
sections are cited by stable ids (`SPEC §machines.calls`). This table maps
every old number to its id and document. Sections added by the split:
§minimal, §introduction, §display-names and §index (with §index.calls and
§index.events) in `SPEC.md`; §content.events; and §content.charges,
§content.unspecified and the same two for page, machines, vector3d and
sound in their documents.

| Old | Id | Document | Title |
|---|---|---|---|
| 1 | §profile | `SPEC.md` | Machine profile 1 |
| 1.1 | §cpu | `SPEC.md` | CPU |
| 1.2 | §memory | `SPEC.md` | Memory map |
| 1.3 | §reset | `SPEC.md` | Reset state |
| 1.4 | §frames | `SPEC.md` | Frames and the budget |
| 1.5 | §stopping | `SPEC.md` | Stopping and faults |
| 2 | §image | `SPEC.md` | Image format |
| 2.1 | §profiles | `SPEC.md` | Machine hosts and profiles |
| 3 | §calls | `SPEC.md` | Calls |
| 3.1 | §calls.convention | `SPEC.md` | Convention |
| 3.2 | §calls.table | `SPEC.md` | Call table |
| 4 | §system | `SPEC.md` | Device 1 — System |
| 5 | §events | `SPEC.md` | Events |
| 6 | §controllers | `SPEC.md` | Controllers |
| 7 | §screen | `SPEC.md` | Screen units |
| 8 | §terminal | `SPEC.md` | Device 2 — Terminal (version 0) |
| 8.1 | §terminal.cells | `SPEC.md` | Cells |
| 8.2 | §terminal.calls | `SPEC.md` | Calls |
| 8.3 | §terminal.put | `SPEC.md` | Put character |
| 8.4 | §terminal.commands | `SPEC.md` | Commands |
| 8.5 | §terminal.reset | `SPEC.md` | Reset state and palette |
| 8.6 | §terminal.presentation | `SPEC.md` | Presentation |
| 9 | §content | `spec/content.md` | Device 4 — Content (version 0) |
| 9.1 | §content.request | `spec/content.md` | Request block |
| 9.2 | §content.submit | `spec/content.md` | Submit and completion |
| 9.3 | §content.status | `spec/content.md` | Status values |
| 9.4 | §content.determinism | `spec/content.md` | Determinism |
| 9.5 | §content.feeds | `spec/content.md` | Feeds, names and search |
| 9.6 | §content.records | `spec/content.md` | Named records |
| 9.7 | §content.documents | `spec/content.md` | Documents |
| 10 | §vector | `SPEC.md` | Device 3 — 2D vector display (version 0) |
| 10.1 | §vector.coordinates | `SPEC.md` | Coordinates |
| 10.2 | §vector.calls | `SPEC.md` | Calls |
| 10.3 | §vector.list | `SPEC.md` | Display list |
| 10.4 | §vector.fill-cap | `SPEC.md` | Fill cap |
| 10.5 | §vector.layers | `SPEC.md` | Layers |
| 10.6 | §vector.phosphor | `SPEC.md` | Phosphor style |
| 10.7 | §vector.determinism | `SPEC.md` | Determinism |
| 11 | §page | `spec/page.md` | Device 7 — Page (version 0) |
| 11.1 | §page.rectangle | `spec/page.md` | Rectangle and scrolling |
| 11.2 | §page.calls | `spec/page.md` | Calls |
| 11.3 | §page.document | `spec/page.md` | Document |
| 11.4 | §page.layout | `spec/page.md` | Layout and presentation |
| 11.5 | §page.events | `spec/page.md` | Events |
| 11.6 | §page.reference | `spec/page.md` | Reference layout |
| 11.7 | §page.determinism | `spec/page.md` | Determinism |
| 12 | §machines | `spec/machines.md` | Device 8 — Machines (version 0) |
| 12.1 | §machines.shell | `spec/machines.md` | The shell and its machines |
| 12.2 | §machines.calls | `spec/machines.md` | Calls |
| 12.3 | §machines.screens | `spec/machines.md` | Screens |
| 12.4 | §machines.input | `spec/machines.md` | Input |
| 12.5 | §machines.frames | `spec/machines.md` | Frames |
| 12.6 | §machines.determinism | `spec/machines.md` | Determinism |
| 12.7 | §machines.snapshots | `spec/machines.md` | Snapshots |
| 13 | §vector3d | `spec/vector3d.md` | Device 9 — 3D vector display (version 0) |
| 13.1 | §vector3d.space | `spec/vector3d.md` | Space and view |
| 13.2 | §vector3d.calls | `spec/vector3d.md` | Calls |
| 13.3 | §vector3d.list | `spec/vector3d.md` | Display list |
| 13.4 | §vector3d.caps | `spec/vector3d.md` | Clipping and caps |
| 13.5 | §vector3d.presentation | `spec/vector3d.md` | Presentation |
| 13.6 | §vector3d.determinism | `spec/vector3d.md` | Determinism |
| 14 | §charges | `SPEC.md` | Charges |
| 15 | §conformance | `SPEC.md` | Conformance |
| 16 | §provisional | `SPEC.md` | Provisional values |
| 17 | §unspecified | `SPEC.md` | Not yet specified |
| 18 | §sound | `spec/sound.md` | Device 6 — Sound (version 0) |
| 18.1 | §sound.output | `spec/sound.md` | Output |
| 18.2 | §sound.calls | `spec/sound.md` | Calls |
| 18.3 | §sound.list | `spec/sound.md` | Command list |
| 18.4 | §sound.voices | `spec/sound.md` | Voices |
| 18.5 | §sound.render | `spec/sound.md` | Rendering a pair |
| 18.6 | §sound.sources | `spec/sound.md` | Sources |
| 18.7 | §sound.presentation | `spec/sound.md` | Presentation |
| 18.8 | §sound.determinism | `spec/sound.md` | Determinism |
