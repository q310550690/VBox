package com.github.catvod.spider;

import android.content.Context;

import com.github.catvod.crawler.Spider;

import java.util.HashMap;
import java.util.List;

/**
 * P2 取流夹具：playUrl 指向测试进程自己起的本地 HTTP 服务（地址由站点的 ext 传入），
 * 用来验证「播放会话租约 → 宿主存活 → 播放器读到字节 → 释放会话」这条链路。
 */
public class StreamSpider extends Spider {

    private String base = "http://127.0.0.1:1";

    @Override
    public void init(Context context, String extend) {
        String e = extend == null ? "" : extend;
        int key = e.indexOf("\"base\"");
        if (key < 0) return;
        int colon = e.indexOf(':', key);
        int q1 = e.indexOf('"', colon + 1);
        int q2 = e.indexOf('"', q1 + 1);
        if (colon > 0 && q1 > 0 && q2 > q1) base = e.substring(q1 + 1, q2);
    }

    @Override
    public String homeContent(boolean filter) {
        return "{\"class\":[{\"type_id\":\"1\",\"type_name\":\"直播\"}]}";
    }

    @Override
    public String categoryContent(String tid, String pg, boolean filter, HashMap<String, String> extend) {
        return "{\"page\":1,\"pagecount\":1,\"limit\":1,\"total\":1,\"list\":["
                + "{\"vod_id\":\"live\",\"vod_name\":\"直播频道\"}]}";
    }

    @Override
    public String detailContent(List<String> ids) {
        return "{\"list\":[{\"vod_id\":\"live\",\"vod_name\":\"直播频道\","
                + "\"vod_play_from\":\"m3u8\",\"vod_play_url\":\"第1集$" + base + "/stream\"}]}";
    }

    @Override
    public String searchContent(String key, boolean quick) {
        return "{\"list\":[]}";
    }

    @Override
    public String playerContent(String flag, String id, List<String> vipFlags) {
        return "{\"parse\":0,\"url\":\"" + base + "/stream\",\"header\":\"\"}";
    }
}
