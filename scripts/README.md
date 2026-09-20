# 脚本使用

## 固定事件演示

```bash
python3 scripts/demo_core.py
```

演示人工改 `Talmey` 为 `Talmy` 时另一块继续出现，并接入机器新增后半句；随后故意制造冲突，检查已修订文字与完整机器候选同时保留。它始终标为模拟。

## 原生模型接口探测

`smoke_qwen.py` 适用于 G0：先准备并启动本地 llama-server，确认已加载匹配 Qwen 模型与 mmproj，并为服务配置独立凭据。启动参数以冻结版本的 `--help` 和官方文档为准，记录准确启动命令；本包不声称已经验证某条模型启动命令。

使用一段具有处理权限的 16 kHz、单声道、16-bit PCM WAV，最长 60 秒。

```bash
python3 scripts/smoke_qwen.py \
  --server http://127.0.0.1:8765 \
  --wav /path/to/authorised-clip.wav \
  --api-key-file /path/to/local-key.txt \
  --model YOUR_SERVER_MODEL_ALIAS \
  --output smoke-result.json
```

`--show-text` 可显示并在结果文件中保存转写。默认只输出时长、哈希、响应长度与单次请求耗时。脚本拒绝远程地址、重定向与代理，端口须与实际服务一致。

脚本只发送一次固定音频请求，G0 需要另外测增长窗口的累计有效 RTF，并验证 Qwen 特殊标签、模板与输出文本解析。收到 JSON 响应仅说明接口返回成功，识别准确率需要核对音频。

本次会话只检查了脚本语法、帮助入口和地址校验；真实模型探测尚未执行。
