# Licensing

Aftermission is licensed under the **GNU Affero General Public License,
version 3.0 only** (`AGPL-3.0-only`); the full text is in
[LICENSE](LICENSE). In particular, the AGPL's network-use clause applies: if
you run a modified version and let users interact with it over a network,
for example as a hosted log viewer, you must offer those users the
corresponding source code.

The web build at <https://poholos.github.io/aftermission/> is Aftermission
itself, under the same license. It is built from this repository, from the
commit its About dialog names, so the source it runs is the source here.

The log parser Aftermission is built on, [dflog](https://github.com/Poholos/dflog),
is a separate project under `MIT OR Apache-2.0`, so it can be used by anyone
in the ArduPilot world without these obligations.

## Commercial license

Aftermission is also available under a separate **commercial license** that
permits use in closed-source products and services without the AGPL's
copyleft obligations. This license can be granted by the copyright holder
(and its successors and assigns). To arrange terms, contact
<info@poholos.com>.

## Contributions

Unless you state otherwise, any contribution you intentionally submit for
inclusion in Aftermission shall be licensed `AGPL-3.0-only`, with no
additional terms or conditions.

In addition, by submitting a contribution to this project (for example, by
opening a pull request), you grant the copyright holder and its successors
and assigns a perpetual, worldwide, non-exclusive, royalty-free, irrevocable
license to use, reproduce, modify, prepare derivative works of, sublicense,
and distribute your contribution, and to relicense it under any terms,
including proprietary and commercial licenses. This lets Aftermission
continue to be offered under the separate commercial license above without
any further agreement or sign-off from you.

## Third-party components

Aftermission depends on third-party open-source components, each under its
own license, which are unaffected by the terms above. The dependency policy
in [deny.toml](deny.toml) allows permissive licenses only, so every
component can travel with either license of Aftermission. Run
`cargo deny check` for a full inventory.
