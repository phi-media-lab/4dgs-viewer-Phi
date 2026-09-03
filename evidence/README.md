# Evidence contract

`remote-native-evidence-v1.schema.json` is the strict structural contract for
reference and comparison receipts emitted by the Remote Frame Mode Player.

The files in `fixtures/` are non-hardware examples used only to
exercise both Schema branches in portable CI. Their adapter, hashes, timings
and media values are placeholders. The reference example is `UNREVIEWED` and
has no neighboring raw frame, so it cannot pass the Player's runtime reference
gate.

Real receipts are valid only together with their referenced raw frames and the
cross-field checks in `../docs/VALIDATION.md`. Store each hardware receipt with
its raw frames and identify the exact source revision it validates.
