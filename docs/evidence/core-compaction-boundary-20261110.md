# Core compaction 边界证据

日期：2026-11-10

## 结论

Lite Core 当前只负责本地 summary/refresh、预算检查和类型化事件。Remote compaction 的路由、模型、协议、授权、网络和 provider receipt 属于 Host/Provider capability；本轮没有把远程协议接入 Core。

## 最小 capability 草案

- 声明：route、model、protocol、最大输入/输出字节和 capability source。
- 请求：source prefix proof、active tail、deadline 和 cancellation。
- 结果：summary、covered prefix proof、provider receipt，以及 usage 或 unknown。
- 失败：unavailable、rejected、timeout、canceled、partial。

未声明 capability 时，Core 使用已有本地路径或类型化停止；事件不能把本地 summary 标成远程压缩。

## 验证

Core library 回归通过，243 passed、1 ignored；现有本地 compaction/refresh 测试继续通过。该证据是接口边界和 fallback 约束，不代表真实 remote provider、网络取消或 partial response 已验收。
