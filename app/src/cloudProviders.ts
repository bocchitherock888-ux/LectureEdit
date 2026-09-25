/** Cloud speech services the app can stream to. Prices and steps follow each provider's own pages (checked 2026-09-25). */
export type CloudEngine = 'doubao' | 'bailian' | 'elevenlabs' | 'soniox';

export interface CloudProvider {
  engine: CloudEngine;
  name: string;
  model: string;
  price: string;
  strength: string;
  keyPlaceholder: string;
  /** Short steps to get a key; `link` names a page the desktop app can open. */
  steps: string[];
  links: { id: string; label: string; url: string }[];
  note?: string;
  regions?: { value: string; label: string }[];
}

export const CLOUD_PROVIDERS: CloudProvider[] = [
  {
    engine: 'doubao',
    name: '豆包语音',
    model: '流式语音识别模型 2.0',
    price: '约 ¥1 / 小时',
    strength: '中文与中英混说识别好，国内首选',
    keyPlaceholder: '粘贴火山引擎豆包语音的 API Key',
    steps: [
      '注册火山引擎并完成实名认证（境外证件需人工审核，约 1–8 个工作日）。',
      '打开豆包语音新版控制台，在「服务管理」里开通「豆包流式语音识别模型 2.0」（小时版）。',
      '进入「API Key 管理」，复制 API Key 粘贴到上面。新版控制台只需要这一个 Key，不需要 App ID 和 Access Token。',
    ],
    links: [{ id: 'doubao-console', label: '打开豆包语音 API Key 页面', url: 'https://console.volcengine.com/speech/new/setting/apikeys' }],
    note: '按识别时长计费，新开通有免费试用额度，以控制台显示为准。',
  },
  {
    engine: 'bailian',
    name: '阿里云百炼',
    model: 'Qwen-Audio 3.1 实时识别',
    price: '按量计费，新用户有免费额度',
    strength: '中英双语提示、支持大量术语，国内外都能用',
    keyPlaceholder: '粘贴以 sk- 开头的百炼 API Key',
    steps: [
      '登录阿里云百炼控制台（中国内地用阿里云账号；海外可用国际站 Model Studio）。',
      '在页面右上角切换到要用的地域，并在下面选择同一个地域：北京或新加坡。Key 只能在创建它的地域使用。',
      '进入「API Key」页面，点「创建 API Key」，复制以 sk- 开头的 Key。首次进入会自动开通模型服务。',
    ],
    links: [
      { id: 'bailian-console-cn', label: '打开百炼控制台（中国内地）', url: 'https://bailian.console.aliyun.com/?tab=model#/api-key' },
      { id: 'bailian-console-intl', label: '打开 Model Studio（国际站）', url: 'https://modelstudio.console.alibabacloud.com/?tab=model#/api-key' },
    ],
    note: '请使用普通的按量付费 Key。Coding Plan 和 Token Plan 的 Key（sk-sp- 开头）按条款不能用于应用程序。',
    regions: [
      { value: 'beijing', label: '北京（中国内地）' },
      { value: 'singapore', label: '新加坡（国际站）' },
    ],
  },
  {
    engine: 'elevenlabs',
    name: 'ElevenLabs',
    model: 'Scribe v2 Realtime',
    price: '约 $0.39 / 小时',
    strength: '海外可用，英文课堂表现好',
    keyPlaceholder: '粘贴 ElevenLabs API Key',
    steps: [
      '登录 elevenlabs.io，在左侧栏点「Developers」，打开「API Keys」。',
      '新建一个 Key，在权限里开启「Speech to Text」（或关闭「Restrict Key」）。',
      '复制 Key 粘贴到上面。Key 只在创建时显示一次。',
    ],
    links: [{ id: 'elevenlabs-console', label: '打开 ElevenLabs API Keys 页面', url: 'https://elevenlabs.io/app/settings/api-keys' }],
    note: '免费账户每月含少量实时转写时长；首次使用可能需要在网站上接受语音转写服务条款。',
  },
  {
    engine: 'soniox',
    name: 'Soniox',
    model: 'stt-rt-v5',
    price: '约 $0.12 / 小时',
    strength: '价格最低，中英混说好，海外可用',
    keyPlaceholder: '粘贴 Soniox API Key',
    steps: [
      '登录 Soniox 控制台。',
      '创建具有「Speech-to-text, real-time」权限的 API Key，复制粘贴到上面。',
    ],
    links: [{ id: 'soniox-console', label: '打开 Soniox 控制台', url: 'https://console.soniox.com' }],
  },
];

export const isCloudEngine = (engine?: string): engine is CloudEngine => CLOUD_PROVIDERS.some((provider) => provider.engine === engine);
export const cloudProvider = (engine?: string) => CLOUD_PROVIDERS.find((provider) => provider.engine === engine);
