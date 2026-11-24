# FlowFile Mac App Store 提交清单

- Bundle ID：`cc.bso.flowfile`
- App Store Connect Apple ID：`6813007595`
- 版本：`1.0.0`
- 首次构建号：`1`；每次重新上传必须递增
- 主分类：工具
- 副分类建议：效率
- 支持网址：`https://github.com/seekos/flowfile/issues`
- 营销网址：`https://github.com/seekos/flowfile`
- 隐私政策网址：`https://github.com/seekos/flowfile/blob/main/PRIVACY.md`
- App Privacy：选择“不从此 App 收集数据”
- 年龄分级：按问卷如实选择；当前功能不包含受限内容
- 出口合规：当前应用不实现自有加密算法；按 App Store Connect 问卷如实回答
- 截图：使用 `app-store/screenshots/` 中 16:10、无透明通道的 PNG
- 审核备注：粘贴 `app-store/app-review-notes.md`
- 发布方式：首次建议“审核通过后手动发布”

## 2026-11-23 实际记录状态

- 已于 2026-11-23 提交 FlowFile macOS 1.0（构建 `1.0.0 (1)`）供 Apple 审核，当前状态为“正在等待审核”
- 已填写副标题、推广文本、描述、关键词、支持网址、营销网址、版权和审核联系信息
- 已完成 App 隐私（不收集数据）、4+ 年龄分级、无第三方内容版权声明、免费价格与全部 175 个国家或地区供应范围
- 中国大陆合规：安全评估与 AIGC 声明均已回答“否”并显示有效
- 欧盟销售：账户仍未完成《数字服务法》交易商状态设置；这可能影响欧盟地区最终供应
- 已上传并处理正式签名构建 `1.0.0 (1)`，已选择到版本 1.0；已上传 2560×1600 Mac 截图
- 已创建 Apple Distribution 证书和 Mac App Store provisioning profile；复用本机有效的 Mac Installer Distribution 身份完成签名
- App Store 构建最低版本为 macOS 12.0，arm64 二进制与 dSYM UUID 一致，签名和安装包校验通过
- 本地沙盒构建已通过启动、文件夹授权和重启后授权恢复测试；169 项自动测试通过，2 项依赖外部环境的测试跳过
- 发布方式为审核通过后手动发布；Apple 提示审核最多可能需要 48 小时，完成后会发送电子邮件通知

构建本地沙盒验证包：

```bash
./scripts/build_app_store.sh --prepare-only -v 1.0.0 -b 1
```

安装本机证书和 provisioning profile 后构建正式上传包：

```bash
FLOWFILE_PROVISIONING_PROFILE="$HOME/Downloads/FlowFile.provisionprofile" \
./scripts/build_app_store.sh -v 1.0.0 -b 1
```

配置 App Store Connect API 凭据后验证并上传：

```bash
FLOWFILE_ASC_KEY_ID="KEY_ID" \
FLOWFILE_ASC_ISSUER_ID="ISSUER_ID" \
./scripts/build_app_store.sh -v 1.0.0 -b 1 --upload
```
