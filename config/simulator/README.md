# 万润 EMS TCP 采集模拟

该模拟器独立于 collector 生产代码，默认关闭，不修改 `config/config.json`。它提供：

- 1 个 PCS：`127.0.0.1:15020` / slave 1；
- 1 个 BAU + 12 个 BCU：`127.0.0.1:15021` / slave 1~13；
- 每个 BCU 的 416 个单体电压和 208 个单体温度；
- PCS/BAU/BCU 主要遥测、心跳变化，以及 Modbus 遥控/遥调写入回读。

## 使用

1. 修改 `wanrun_modbus_simulator.json` 中的 `enabled` 为 `true`。
2. 启动模拟器：

   ```bash
   python3 scripts/wanrun_modbus_simulator.py
   ```

3. 启动 collector，指向自动生成的独立配置：

   ```bash
   cargo run -p collector-cmd -- --config config/simulator/generated/config_万润_EMS_模拟.json
   ```

collector 仍使用原有 PCS/BAU/BCU Excel 点表、`projects/万润.lua` 和 MQTT Topic，因此可以验证完整业务链路，而不是绕过采集层直接造 MQTT 数据。

BAU 模拟配置使用 `BAU_协能_模拟.xlsx`。原生产点表存在两处地址重叠：点 10/11 同为 4108，点 53 数量为 2 会覆盖点 54。模拟副本仅禁用地址不明的点 11，并将 U16 的点 53 数量改为 1；生产点表保持不变，待现场协议确认后再修正。

只校验配置并生成 collector 模拟配置，不监听端口：

```bash
python3 scripts/wanrun_modbus_simulator.py --check
```

停用时将 `enabled` 改回 `false`。即使误执行脚本，也不会启动 TCP 服务。
