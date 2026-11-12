# Issuer 测试

集成测试在进程内启动临时 Signer，并使用独立 SQLite 数据库验证：

- 单 Realm 注册；
- Publisher/package 与 linked workload proof；
- Actor ID 分配和 collision ledger；
- credential issuer/provider/audience/expiry；
- renewal possession、rotation、replay 与 revocation；
- `actr-package/2` 原始 manifest/signature 的 SDK HTTP 透传、无 target 注册、续期/重签及旧 target 清理；
- v2/旧格式混用、错误来源、设备/Owner 授权与不降级回归；
- Signer 不可用和非法配置的 fail-closed 行为。

这些测试不依赖外部 Signer，也不能替代独立安全审查。

`v2_package` 使用 `crates/actr-pack/tests/fixtures/registration-v2.toml` 作为与 Host 任务 #422
共享的 unsigned 模板，由真实 packer/verifier 生成公开签名材料。空 Component 只用于容器和注册测试，
不作为实际 Workload 执行验收。完整 `actr run → register → RPC` 由 #422 的 Host 集成继续覆盖。
