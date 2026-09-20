# 参考逻辑的用途与范围

`core.py` 演示保守三路合并、持续机器快照、固定编辑草稿、人工并发检查、幂等提交、撤销、候选处理与稳定资料锚点。`tests/test_core.py` 覆盖这些规则，`scripts/demo_core.py` 用固定事件展示完整顺序。

参考程序只使用 Python 标准库，数据存在内存中。`dumps/loads` 用于恢复语义测试；实际桌面应用的持久化由 SQLite 事务实现。模拟的 recording 标志和采样计数不会调用麦克风。

## 合并保护

`merge3` 对同一基准产生的词项差异做结构合并。重叠修改、插入边界冲突和重复定位歧义返回完整候选。机器新增续文可以与之前的人工纠错合并。

`Segment.projection` 持续保留原人工基准；模型恰好识别对了也保留人工保护。后续冲突沿用最近安全展示，使已接入的续文继续可见。再次编辑采用保守累计基准，生产实现可使用 protected edits 降低额外冲突。

## 生产迁移要求

迁移到 Rust 时实现完整的 grapheme 范围、编辑器 selection 映射、IME、受保护修改来源、事务和事件日志。`protected_edits` 表与 TypeScript 类型提供初始入口。缓存展示投影，避免每次绘制都回放全部机器历史。

参考程序没有实现原生录音、实际模型、增量音频协议、落盘调度、完整重做、跨窗口事件同步或安全导入。其输出只用于规格验证。

```bash
python3 -m unittest discover -s tests -v
python3 scripts/demo_core.py
```
