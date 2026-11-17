/** W14 §6.8 云盘搜索错误码（后端以 `code：message` 形式返回）。 */

export type CloudSearchCode =
  | 'cloudsearch.partial'
  | 'cloudsearch.source_unavailable'
  | 'cloudsearch.all_sources_failed'
  | 'cloudsearch.pansou_invalid'
  | 'cloudsearch.timeout'

const CODE_PATTERN = /cloudsearch\.(?:partial|source_unavailable|all_sources_failed|pansou_invalid|timeout)/

/** 从后端错误消息里提取错误码；没有则返回空串。 */
export function detectCloudSearchCode(message: string): CloudSearchCode | '' {
  const match = message.match(CODE_PATTERN)
  return (match?.[0] as CloudSearchCode | undefined) ?? ''
}

export interface CloudSearchHint {
  /** 面向用户的动作提示；空串表示用原始错误文案。 */
  text: string
  action: 'settings' | 'retry' | ''
}

/** 错误码 → 提示文案与动作（W14 §6.8 前端表现）。 */
export function cloudSearchHint(code: CloudSearchCode | ''): CloudSearchHint {
  switch (code) {
    case 'cloudsearch.source_unavailable':
      return { text: '内置搜索源暂不可用：请检查网络/代理，或配置 PanSou 增强源。', action: 'settings' }
    case 'cloudsearch.all_sources_failed':
      return { text: '内置与增强搜索源均失败，请检查网络后重试。', action: 'retry' }
    case 'cloudsearch.pansou_invalid':
      return { text: '增强源返回结构异常：请检查 PanSou 服务地址或版本。', action: 'settings' }
    case 'cloudsearch.timeout':
      return { text: '搜索超时，可重试或稍后再试。', action: 'retry' }
    case 'cloudsearch.partial':
      return { text: '部分来源超时，已展示可用结果。', action: 'retry' }
    default:
      return { text: '', action: '' }
  }
}
