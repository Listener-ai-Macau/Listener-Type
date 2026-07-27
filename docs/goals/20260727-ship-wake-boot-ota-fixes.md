# Goal：把唤醒 / 有线 Boot / OTA 重连修进可装包（收口）

> Active Goal。代码已在 master（`4d59440` + `bf93764`），但 **已发布 MSI 仍是旧包** → owner 装上/打开的还是坏体感。本 Goal 打通到可装包 + 本机验证。

## 问题

1. 唤醒词灵敏度：旧校准 1.5/0.25 压死 bootstrap → 代码已修，**安装包未更新**  
2. 有线 Boot：应自动检查/修复，不要单独按钮 → 代码已修，**安装包未更新**  
3. OTA 后连不上 Type：等太短 → 代码已修，**安装包未更新**  

## 成功标准

| # | 检查 | 判定 |
|---|---|---|
| 1 | 当前 HEAD 全量打 MSI（无 ReuseExistingExe） | MSI 落盘、版本 1.0.3 |
| 2 | msiexec 安装 + 安装 exe 哈希对齐 | exit 0 |
| 3 | 本机 calibration 为 3.0/0.08；启动日志含 `runtime KWS config … score=3.0 threshold=0.08` | 日志 |
| 4 | 安装后二进制含 boot 自动探测 / 无 UI Boot 修复依赖（源码门禁 + 可选字符串） | 测试/文件 |
| 5 | MSI 拷 Denzic 根 + `check:release-root-artifacts` | PASS |
| 6 | 证据 `docs/goals/20260727-ship-wake-boot-ota-fixes.evidence.md` | 存在 |
| 7 | （可选）覆盖 GitHub Release v1.0.3 资产 | 有上传 |

## 人工边界

- 真机喊词 / 真 OTA 若环境不允许：写清替代证据；不装 PASS。  
- 不改产品语义，只交付当前 master 修复包。

## 机器闭环

输入：`master` HEAD。输出：新 MSI + 根目录校验 + 安装日志 + evidence。
