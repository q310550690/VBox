package com.github.catvod.spider;

import android.content.Context;

import com.github.catvod.crawler.Spider;

import java.util.HashMap;
import java.util.List;

/**
 * P1 联调夹具：不依赖任何外网，返回固定结构的 catvod JSON，
 * 用来验证「宿主常驻协议 → Rust 客户端 → CLI」这条链路本身。
 *
 * <p>注意：这是测试代码，不随包分发（只在 `node scripts/build-jvm.mjs --fixtures` 时编译）。
 */
public class TestSpider extends Spider {

    private String ext = "";

    @Override
    public void init(Context context, String extend) {
        this.ext = extend == null ? "" : extend;
    }

    @Override
    public String homeContent(boolean filter) {
        return "{\"class\":[{\"type_id\":\"1\",\"type_name\":\"电影\"},"
                + "{\"type_id\":\"2\",\"type_name\":\"剧集\"}],\"filters\":{}}";
    }

    @Override
    public String categoryContent(String tid, String pg, boolean filter, HashMap<String, String> extend) {
        return "{\"page\":" + pg + ",\"pagecount\":3,\"limit\":20,\"total\":60,\"list\":["
                + "{\"vod_id\":\"" + tid + "-" + pg + "-1\",\"vod_name\":\"测试片名A\","
                + "\"vod_pic\":\"http://example.com/a.jpg\",\"vod_remarks\":\"HD\"}]}";
    }

    @Override
    public String detailContent(List<String> ids) {
        String id = ids == null || ids.isEmpty() ? "" : ids.get(0);
        return "{\"list\":[{\"vod_id\":\"" + id + "\",\"vod_name\":\"测试详情\","
                + "\"vod_pic\":\"http://example.com/d.jpg\",\"vod_content\":\"简介\","
                + "\"vod_play_from\":\"m3u8\",\"vod_play_url\":\"第1集$http://example.com/1.m3u8\"}]}";
    }

    @Override
    public String searchContent(String key, boolean quick) {
        return "{\"list\":[{\"vod_id\":\"s-1\",\"vod_name\":\"" + key + " 结果\"}]}";
    }

    @Override
    public String playerContent(String flag, String id, List<String> vipFlags) {
        return "{\"parse\":0,\"url\":\"http://example.com/play.m3u8\",\"header\":\"\"}";
    }
}
