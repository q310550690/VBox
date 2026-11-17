// 云盘账号状态（W14 §7.3）：账号列表 / 登录态 / provider 能力，供云盘首页、管理页与账号面板共用。
// 注意：真实登录流程（扫码/设备码/网页授权）仍在 AccountPanel 内驱动，成功后调用 refresh()。
import { defineStore } from 'pinia'
import * as api from '@/api'
import type { PanAccountView, PanLoginProvider } from '@/types'

/** provider 展示顺序（与 W13 账号面板一致） */
export const CLOUD_PROVIDER_ORDER = ['p115', 'quark', 'xunlei', 'ali', 'uc', 'pikpak'] as const

/** provider 展示名 */
export const CLOUD_PROVIDER_NAME: Record<string, string> = {
  p115: '115 网盘',
  quark: '夸克网盘',
  xunlei: '迅雷云盘',
  ali: '阿里云盘',
  uc: 'UC 网盘',
  pikpak: 'PikPak',
}

export const useCloudAccountsStore = defineStore('cloudAccounts', {
  state: () => ({
    accounts: [] as PanAccountView[],
    providers: [] as PanLoginProvider[],
    loading: false,
    loaded: false,
    error: '',
  }),
  getters: {
    /** provider → 当前账号（每个 provider 只保留一条活跃账号） */
    byProvider(state): Record<string, PanAccountView> {
      const map: Record<string, PanAccountView> = {}
      for (const account of state.accounts) map[account.provider] = account
      return map
    },
    /** 云盘首页胶囊：只渲染已连接/已过期账号，按固定 provider 顺序 */
    pills(state): PanAccountView[] {
      const order = CLOUD_PROVIDER_ORDER as readonly string[]
      return state.accounts
        .filter((item) => item.status === 'connected' || item.status === 'expired')
        .sort((a, b) => order.indexOf(a.provider) - order.indexOf(b.provider))
    },
    /** 有有效凭据的账号（默认打开管理页时优先使用） */
    connected(state): PanAccountView[] {
      return state.accounts.filter((item) => item.status === 'connected')
    },
    providerName(): (provider: string) => string {
      return (provider: string) => CLOUD_PROVIDER_NAME[provider] ?? provider
    },
  },
  actions: {
    /** 拉取账号列表与 provider 能力；demoMode 下保持空列表 */
    async load(force = false) {
      if (this.loading) return
      if (!force && this.loaded) return
      if (api.demoMode) {
        this.loaded = true
        return
      }
      this.loading = true
      this.error = ''
      try {
        const [providers, accounts] = await Promise.all([
          api.panLoginProviders(),
          api.panAccountsList(),
        ])
        this.providers = providers
        this.accounts = accounts
        this.loaded = true
      } catch (e) {
        this.accounts = []
        this.error = api.formatError(e)
      } finally {
        this.loading = false
      }
    },
    /** 登录/登出/验证后强制刷新 */
    async refresh() {
      await this.load(true)
    },
    async remove(accountId: string) {
      await api.panAccountRemove(accountId)
      this.accounts = this.accounts.filter((item) => item.account_id !== accountId)
    },
    async verify(provider: string): Promise<boolean> {
      const ok = await api.panCredentialsVerify(provider)
      await this.refresh()
      return ok
    },
  },
})
