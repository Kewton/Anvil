# Design Note

Issue #903 asks for obligation-scoped correction.

The current tree already carries `CorrectionKind`, `RepairPacket`, and obligation targets. This PR extends the model with test-correction and manifest-correction classifications so generated-test bugs and invalid manifests can route away from generic implementation patch repair.

