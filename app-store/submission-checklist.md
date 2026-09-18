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

## 2026-09-17 实际记录状态

- 已创建：FlowFile macOS 1.0，状态为“准备提交”
- 待填写：副标题、版本描述、关键词、支持网址、营销网址、版权
- 待完成：App 隐私问卷、年龄分级、内容版权、价格与销售范围
- 待上传：正式签名构建和 Mac 截图
- 账户提醒：App Store Connect 显示尚未提供欧盟《数字服务法》交易商状态；若要在欧盟销售，需要账户持有人完成该项验证
- 当前本机只有 Apple Distribution 公钥证书，钥匙串中没有可用于签名的私钥，也没有 Mac Installer Distribution 身份；正式 `.pkg` 暂时无法生成

构建本地沙盒验证包：

```bash
./scripts/build_app_store.sh --prepare-only -v 1.0.0 -b 1
```

安装本机证书后构建正式上传包：

```bash
./scripts/build_app_store.sh -v 1.0.0 -b 1
```

配置 App Store Connect API 凭据后验证并上传：

```bash
FLOWFILE_ASC_KEY_ID="KEY_ID" \
FLOWFILE_ASC_ISSUER_ID="ISSUER_ID" \
./scripts/build_app_store.sh -v 1.0.0 -b 1 --upload
```
