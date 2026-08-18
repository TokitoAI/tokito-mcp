# Third-party content notice

## KiCad Symbol Libraries

The catalog packs served by this service include symbol data derived from the official KiCad Symbol Libraries (https://gitlab.com/kicad/libraries/kicad-symbols), copyright © The KiCad Developers and the KiCad library contributors.

The KiCad Symbol Libraries are licensed under the Creative Commons Attribution-Share Alike 4.0 International license (CC-BY-SA-4.0), with the KiCad Libraries Exception (https://www.kicad.org/libraries/license/). The exception waives attribution/share-alike obligations for electronic designs and files generated from the libraries; it does not apply to redistribution of the libraries themselves.

This service redistributes that symbol data in modified form: symbols are converted to the `.tokito_sym` s-expression format and packaged into SQLite catalog packs (`symbols.sqlite`, see `pack` in the [Workspace](README.md#workspace) section). Accordingly, the KiCad-derived symbol data in those packs — including our modifications to it — is made available under CC-BY-SA-4.0. Designs created by end users with these symbols remain covered by the KiCad Libraries Exception and carry no attribution or share-alike obligation.

The server source code in this repository is separately licensed; see [LICENSE](LICENSE).
