# -*- coding: utf-8 -*-
"""VBox Python spider sidecar.

由 Rust 侧（`crates/vbox_core/src/python/mod.rs`）以「每次调用一个子进程」的方式执行：

    <python> host.py
    stdin : {"script": "/abs/path/x.py", "ext": "...", "method": "homeContent", "args": [...]}
    stdout: 一行 JSON {"ok": true, "result": ...} 或 {"ok": false, "error": "...", "trace": "..."}

约定：
- 脚本的 `print()` 输出全部重定向到 stderr，保证 stdout 只有一行协议 JSON；
- 注入 `base` / `base.spider` / `base.localProxy` 模块，等价于 TVBox pyramid 的
  `base` 包，因此 `from base.spider import Spider` 的 Python spider 可直接运行；
- 方法签名对齐 pyramid `app.py`：
    init(extend)
    homeContent(filter)
    categoryContent(tid, pg, filter, extend)
    detailContent(ids)              # ids 是 list
    searchContent(key, quick, pg="1")
    playerContent(flag, id, vipFlags)
    liveContent(url)
  子类少写参数（如 `homeContent(self)`）或把 `pg` 设为必填都能兼容；
- 返回 dict/list 时原样 JSON 化；返回 str 时保持字符串（Rust 侧会再尝试按 JSON 解析）。

注意：CPython sidecar 不是安全沙箱 —— 脚本拥有当前用户的文件/网络权限。VBox 的
安全边界只在「不执行 jar / dex」，Python 脚本请只使用可信配置来源。
"""

import importlib.machinery
import importlib.util
import inspect
import json
import os
import sys
import traceback
import types

# 协议输出用的真实 stdout；脚本可见的 sys.stdout 会被重定向到 stderr
_REAL_STDOUT = sys.stdout
_REAL_STDERR = sys.stderr

# 缓存（getCache/setCache）在本进程内有效；TVBox 原实现走本地 9978 服务
_CACHE = {}

HOME_CONTENT = "homeContent"
HOME_VIDEO_CONTENT = "homeVideoContent"
CATEGORY_CONTENT = "categoryContent"
DETAIL_CONTENT = "detailContent"
SEARCH_CONTENT = "searchContent"
PLAYER_CONTENT = "playerContent"
LIVE_CONTENT = "liveContent"


def _json_default(obj):
    """把不可 JSON 化的对象降级成字符串，避免整次调用失败。"""
    try:
        return str(obj)
    except Exception:
        return None


def _write_protocol(payload):
    data = json.dumps(payload, ensure_ascii=True, default=_json_default)
    _REAL_STDOUT.write(data + "\n")
    _REAL_STDOUT.flush()


class Spider(object):
    """`base.spider.Spider` 的最小兼容实现。

    只依赖标准库；`requests` / `lxml` 在使用到对应 helper 时才延迟导入，
    这样无第三方依赖的脚本也能运行。
    """

    _instance = None

    def __new__(cls, *args, **kwargs):
        if cls._instance is not None:
            return cls._instance
        cls._instance = super(Spider, cls).__new__(cls)
        return cls._instance

    def __init__(self):
        self.extend = ""

    # ---------- 生命周期 ----------

    def init(self, extend=""):
        pass

    def destroy(self):
        pass

    def getName(self):
        return type(self).__name__

    def getDependence(self):
        return []

    def setExtendInfo(self, extend):
        self.extend = extend if extend is not None else ""

    # ---------- 子类覆盖的内容接口（基类空实现） ----------

    def homeContent(self, filter):
        return None

    def homeVideoContent(self):
        return None

    def categoryContent(self, tid, pg, filter, extend):
        return None

    def detailContent(self, ids):
        return None

    def searchContent(self, key, quick, pg="1"):
        return None

    def playerContent(self, flag, id, vipFlags):
        return None

    def liveContent(self, url):
        return None

    def localProxy(self, param):
        return None

    def isVideoFormat(self, url):
        return False

    def manualVideoCheck(self):
        return False

    def action(self, action):
        return None

    # ---------- 常用 helper（对齐 pyramid base/spider.py） ----------

    def log(self, msg):
        try:
            if isinstance(msg, (dict, list)):
                _REAL_STDERR.write(json.dumps(msg, ensure_ascii=False) + "\n")
            else:
                _REAL_STDERR.write(str(msg) + "\n")
            _REAL_STDERR.flush()
        except Exception:
            pass

    def fetch(self, url, headers=None, cookies=None, params=None, timeout=15,
              verify=True, stream=False, allow_redirects=True):
        import requests
        rsp = requests.get(url, params=params, cookies=cookies, headers=headers,
                           timeout=timeout, verify=verify, stream=stream,
                           allow_redirects=allow_redirects)
        try:
            rsp.encoding = "utf-8"
        except Exception:
            pass
        return rsp

    def post(self, url, data=None, headers=None, cookies=None, params=None,
             json=None, timeout=15, verify=True, stream=False, allow_redirects=True):
        import requests
        rsp = requests.post(url, params=params, data=data, json=json,
                            cookies=cookies, headers=headers, timeout=timeout,
                            verify=verify, stream=stream,
                            allow_redirects=allow_redirects)
        try:
            rsp.encoding = "utf-8"
        except Exception:
            pass
        return rsp

    def postJson(self, url, json, headers=None, cookies=None):
        import requests
        return requests.post(url, json=json, headers=headers, cookies=cookies)

    def html(self, content):
        from lxml import etree
        return etree.HTML(content)

    def xpText(self, root, expr):
        try:
            ele = root.xpath(expr)
            return ele[0] if len(ele) > 0 else ""
        except Exception:
            return ""

    def regStr(self, src, reg=None, group=1):
        if reg is None:
            return ""
        import re
        match = None
        try:
            match = re.search(reg, src)
        except Exception:
            pass
        if not match:
            try:
                match = re.search(src, reg)
            except Exception:
                pass
        return match.group(group) if match else ""

    def removeHtmlTags(self, src):
        import re
        return re.sub(re.compile("<.*?>"), "", src)

    def cleanText(self, src):
        import re
        return re.sub(
            "[\U0001F600-\U0001F64F\U0001F300-\U0001F5FF"
            "\U0001F680-\U0001F6FF\U0001F1E0-\U0001F1FF]",
            "",
            src,
        )

    def str2json(self, content):
        return json.loads(content)

    def json2str(self, content):
        return json.dumps(content, ensure_ascii=False)

    def getProxyUrl(self, local=True):
        # 桌面端没有 9978 本地代理；返回空串，脚本通常只把它拼进 url
        return ""

    def loadModule(self, name, fileName=None):
        path = fileName if fileName else name
        if not os.path.isabs(path):
            path = os.path.join(os.path.dirname(os.path.abspath(sys.argv[0])), path)
        module = _load_module(path, "vbox_py_plugin_%d" % (abs(hash(path)) % 1000000))
        return module

    def loadSpider(self, name):
        return self.loadModule(name).Spider()

    def getCache(self, key):
        return _CACHE.get(key)

    def setCache(self, key, value):
        _CACHE[key] = value
        return "succeed"

    def delCache(self, key):
        _CACHE.pop(key, None)
        return "succeed"


class Proxy(object):
    """`base.localProxy.Proxy` 的空实现（桌面端没有 9978 服务）。"""

    @staticmethod
    def getUrl(spider=None, local=True):
        return ""

    @staticmethod
    def getPort(spider=None):
        return 0


def _install_base_modules():
    """注入 `base` / `base.spider` / `base.localProxy` 三个模块。"""
    base_mod = sys.modules.get("base")
    if base_mod is not None and getattr(base_mod, "__vbox_injected__", False):
        return

    base_mod = types.ModuleType("base")
    base_mod.__vbox_injected__ = True
    base_mod.__path__ = []  # 声明为 package，`from base.spider import Spider` 才认

    spider_mod = types.ModuleType("base.spider")
    spider_mod.Spider = Spider
    spider_mod.__all__ = ["Spider"]

    local_proxy_mod = types.ModuleType("base.localProxy")
    local_proxy_mod.Proxy = Proxy
    local_proxy_mod.__all__ = ["Proxy"]

    base_mod.spider = spider_mod
    base_mod.localProxy = local_proxy_mod

    sys.modules["base"] = base_mod
    sys.modules["base.spider"] = spider_mod
    sys.modules["base.localProxy"] = local_proxy_mod


def _load_module(script_path, module_name):
    loader = importlib.machinery.SourceFileLoader(module_name, script_path)
    spec = importlib.util.spec_from_file_location(module_name, script_path, loader=loader)
    if spec is None:
        raise RuntimeError("无法为脚本创建 import spec: %s" % script_path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


def _positional_params(fn):
    try:
        signature = inspect.signature(fn)
    except (TypeError, ValueError):
        return None
    return [
        p for p in signature.parameters.values()
        if p.kind in (inspect.Parameter.POSITIONAL_ONLY,
                      inspect.Parameter.POSITIONAL_OR_KEYWORD)
    ]


def _has_var_positional(fn):
    try:
        signature = inspect.signature(fn)
    except (TypeError, ValueError):
        return True
    return any(p.kind == inspect.Parameter.VAR_POSITIONAL
               for p in signature.parameters.values())


def _call_with_signature(fn, args):
    """按函数真实签名裁剪/补齐参数，兼容少写或多写参数的老脚本。"""
    params = _positional_params(fn)
    if params is None:
        return fn(*args)
    if _has_var_positional(fn):
        return fn(*args)

    call_args = list(args[: len(params)])
    for param in params[len(call_args):]:
        if param.default is not inspect.Parameter.empty:
            call_args.append(param.default)
        elif param.name in ("pg", "page", "p"):
            call_args.append("1")
        else:
            # 必填但未提供：交给 Python 抛 TypeError（错误信息会回传前端）
            break
    return fn(*call_args)


def _instantiate(module):
    cls = getattr(module, "Spider", None)
    if isinstance(cls, type) and cls is not Spider:
        return cls()

    instance = getattr(module, "spider", None)
    if instance is not None and not isinstance(instance, type):
        return instance

    raise RuntimeError(
        "脚本未定义 class Spider（需要 `from base.spider import Spider` + "
        "`class Spider(Spider)`）"
    )


def _is_implemented(spider, method):
    """方法是否被脚本真正实现（基类 `base.spider.Spider` 的同名空方法不算）。"""
    if not hasattr(spider, method):
        return False
    # 实例属性注入（少数脚本用 self.xxx = ... 动态绑定）
    if method in getattr(spider, "__dict__", {}):
        return True
    impl = getattr(type(spider), method, None)
    base = getattr(Spider, method, None)
    return impl is not None and impl is not base


def _call_init(spider, extend):
    init = getattr(spider, "init", None)
    if not callable(init):
        return
    params = _positional_params(init)
    if params is None or len(params) >= 1 or _has_var_positional(init):
        _call_with_signature(init, [extend])
    else:
        init()


def _dispatch(request):
    script_path = request.get("script") or ""
    ext = request.get("ext")
    if ext is None:
        ext = ""
    method = request.get("method") or HOME_CONTENT
    args = request.get("args")
    if not isinstance(args, list):
        args = []

    script_path = os.path.abspath(script_path)
    if not os.path.isfile(script_path):
        raise RuntimeError("脚本文件不存在: %s" % script_path)

    script_dir = os.path.dirname(script_path)
    if script_dir not in sys.path:
        sys.path.insert(0, script_dir)
    sys.argv = [script_path]

    _install_base_modules()

    module_name = "vbox_py_spider_%d" % (abs(hash(script_path)) % 10000000)
    module = _load_module(script_path, module_name)

    spider = _instantiate(module)
    _call_init(spider, ext)

    if not _is_implemented(spider, method):
        return {"missing": True,
                "error": "脚本 %s 未实现 %s() 方法" % (module_name, method)}

    fn = getattr(spider, method, None)
    if not callable(fn):
        return {"missing": True,
                "error": "脚本 %s 的 %s 不是可调用方法" % (module_name, method)}

    result = _call_with_signature(fn, args)
    return {"missing": False, "result": result}


def main():
    # 脚本的 print 全部走 stderr，stdout 只留给协议
    try:
        sys.stdout = sys.stderr
    except Exception:
        pass
    sys.dont_write_bytecode = True

    try:
        raw = sys.stdin.buffer.read()
        request = json.loads(raw.decode("utf-8", "replace"))
    except Exception as exc:
        _write_protocol({"ok": False, "error": "host 协议解析失败: %s: %s"
                         % (type(exc).__name__, exc)})
        return 2

    try:
        outcome = _dispatch(request)
    except BaseException as exc:  # 包含 SystemExit，脚本主动退出也要回传错误
        trace = traceback.format_exc()
        _write_protocol({
            "ok": False,
            "missing": False,
            "error": "%s: %s" % (type(exc).__name__, exc),
            "trace": trace[-6000:],
        })
        return 1

    if outcome.get("missing"):
        _write_protocol({"ok": False, "missing": True, "error": outcome.get("error")})
        return 0

    _write_protocol({"ok": True, "result": outcome.get("result")})
    return 0


if __name__ == "__main__":
    sys.exit(main())
