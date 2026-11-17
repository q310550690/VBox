// 网盘账号登录（W13）：Tauri 命令封装。
//
// 安全边界：这些接口返回的都是脱敏数据（票据不含 uid/sign/device_code，
// 账号摘要不含 cookie/token）；凭据只留在后端 DPAPI 加密文件里。
import { invoke } from '@tauri-apps/api/core'
import type {
  PanAccountView,
  PanLoginPollResult,
  PanLoginProvider,
  PanQrTicket,
} from '@/types'

/** 各 provider 的登录能力（协议登录 / 网页授权） */
export function panLoginProviders(): Promise<PanLoginProvider[]> {
  return invoke<PanLoginProvider[]>('login_providers')
}

/** 开始一次协议登录：返回二维码 / 设备码票据 */
export function panLoginStart(provider: string): Promise<PanQrTicket> {
  return invoke<PanQrTicket>('login_start', { provider })
}

/** 轮询登录状态；成功时凭据已由后端加密落盘 */
export function panLoginPoll(provider: string, ticketId: string): Promise<PanLoginPollResult> {
  return invoke<PanLoginPollResult>('login_poll', { provider, ticketId })
}

/** 取消一次进行中的登录 */
export function panLoginCancel(ticketId: string): Promise<boolean> {
  return invoke<boolean>('login_cancel', { ticketId })
}

/** 手动写入 cookie / token（第三方工具导出或无法扫码时的兜底） */
export function panLoginManual(payload: {
  provider: string
  cookie?: string
  token?: string
  refreshToken?: string
  displayName?: string
}): Promise<PanAccountView> {
  return invoke<PanAccountView>('login_manual', payload)
}

/** 已保存账号列表 */
export function panAccountsList(): Promise<PanAccountView[]> {
  return invoke<PanAccountView[]>('accounts_list')
}

/** 删除一个账号 */
export function panAccountRemove(accountId: string): Promise<boolean> {
  return invoke<boolean>('account_remove', { accountId })
}

/** 退出某 provider 的全部账号 */
export function panCredentialsClear(provider: string): Promise<number> {
  return invoke<number>('credentials_clear', { provider })
}

/** 当前是否有可用凭据（本地过期判断） */
export function panCredentialsVerify(provider: string): Promise<boolean> {
  return invoke<boolean>('credentials_verify', { provider })
}

/** 打开网页登录窗口，用户登录后从 WebView 取回 Cookie（返回 null 表示取消） */
export function panOpenWebLogin(provider: string, timeoutSecs?: number): Promise<PanAccountView | null> {
  return invoke<PanAccountView | null>('open_web_login', { provider, timeoutSecs })
}