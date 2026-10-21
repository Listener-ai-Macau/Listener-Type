// Official setup/billing references checked 2026-10-21. No credentials, requests,
// model defaults or prices are derived from these links.
export const TEXT_PROVIDER_SETUP_LINKS: Readonly<Record<string, { setup: string; pricing: string }>> = {
  ark: {
    setup: 'https://console.volcengine.com/ark/region:cn-beijing/docs/manage-api-keys',
    pricing: 'https://console.volcengine.com/ark/region:cn-beijing/docs/model-service-pricing',
  },
  deepseek: {
    setup: 'https://api-docs.deepseek.com/',
    pricing: 'https://api-docs.deepseek.com/quick_start/pricing/',
  },
  siliconflow: {
    setup: 'https://docs.siliconflow.cn/docs/userguide/quickstart',
    pricing: 'https://cloud.siliconflow.cn/models',
  },
  openai: {
    setup: 'https://developers.openai.com/api/docs/quickstart',
    pricing: 'https://developers.openai.com/api/docs/pricing',
  },
  gemini: {
    setup: 'https://ai.google.dev/gemini-api/docs/get-started',
    pricing: 'https://ai.google.dev/gemini-api/docs/pricing',
  },
};
