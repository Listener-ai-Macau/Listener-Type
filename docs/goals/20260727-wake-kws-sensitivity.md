# Goal：唤醒词灵敏度（KWS）优化

## 目标

解决 owner 反馈「唤醒词灵敏度不太行」：主路径不得被声纹录入时写入的**最严** KWS 校准（常为 1.5 / 0.25）锁死；运行时固定产品敏感 bootstrap **3.0 / 0.08**。

## 成功标准

1. `configured_keyword_values` 运行时返回 bootstrap，不读严格 pin  
2. `persist_bootstrap_calibration_if_missing` 会升级已有更严校准文件  
3. `calibrate` 仍验证样本含唤醒词，但落盘为 bootstrap  
4. `cargo test --lib wake_phrase` PASS  
5. 本机 calibration.json 升级为 3.0 / 0.08；Type 用当前树二进制启动后日志出现 `score=3.0 threshold=0.08`

## 防回退

- 主路径仍 `StreamingDetector::new`（含短前缀变体）  
- 无声纹 open-gate 不变  
- `new_strict` 不进主唤醒  
