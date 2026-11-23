package com.github.catvod.spider;

import com.github.catvod.crawler.Spider;

/**
 * 失败路径夹具：`homeContent` 抛异常，验证宿主 ok=false + 异常文本的信封，
 * 以及 Rust 侧把它归成可读错误（而不是超时）。
 */
public class FailSpider extends Spider {

    @Override
    public String homeContent(boolean filter) {
        throw new IllegalStateException("fixture: 故意失败");
    }
}
