static double qpc_ms(void) {
    static double inv = 0;
    LARGE_INTEGER f, c;
    if (inv == 0) { QueryPerformanceFrequency(&f); inv = 1000.0 / (double)f.QuadPart; }
    QueryPerformanceCounter(&c);
    return (double)c.QuadPart * inv;
}

// Decodes one access unit to BGR24. When the decoded picture is larger than the box
// fit_w x fit_h (0 = no limit) it is downscaled to fit (aspect kept, never upscaled) in the same
// swscale pass as the YUV->BGR conversion, so a 4K stream never exists as a 4K BGR frame.
// t_ms (3 doubles, may be NULL) accumulates: [0] decoder, [1] GPU->CPU download, [2] convert+scale.
int dec_decode(void *h, const uint8_t *data, int len, uint8_t *out, int maxsize, int fit_w, int fit_h,
               double *t_ms, int *w, int *h_out, int *hw_used) {
    Dec *d = h;
    if (!d) return -1;
    int produced = 0;
    double t0 = qpc_ms();
    d->pkt->data = (uint8_t *)data;
    d->pkt->size = len;
    int ret = avcodec_send_packet(d->ctx, d->pkt);
    if (ret < 0) return ret;
    for (;;) {
        ret = avcodec_receive_frame(d->ctx, d->frame);
        if (t_ms) t_ms[0] += qpc_ms() - t0;
        if (ret == AVERROR(EAGAIN) || ret == AVERROR_EOF) break;
        if (ret < 0) return ret;
        double t1 = qpc_ms();
        AVFrame *src = d->frame;
        *hw_used = 0;
        if (d->frame->format == d->hw_fmt && d->hw_fmt != 0) {
            av_frame_unref(d->swframe);
            ret = av_hwframe_transfer_data(d->swframe, d->frame, 0);
            if (ret < 0) { av_frame_unref(d->frame); return ret; }
            src = d->swframe;
            *hw_used = 1;
        }
        double t2 = qpc_ms();
        if (t_ms) t_ms[1] += t2 - t1;
        int fw = src->width, fh = src->height;
        int dw = fw, dh = fh;
        if (fit_w > 0 && fit_h > 0 && (fw > fit_w || fh > fit_h)) {
            if ((int64_t)fw * fit_h > (int64_t)fh * fit_w) { dw = fit_w; dh = (int)((int64_t)fh * fit_w / fw); }
            else { dh = fit_h; dw = (int)((int64_t)fw * fit_h / fh); }
            dw &= ~1; dh &= ~1;
            if (dw < 2) dw = 2;
            if (dh < 2) dh = 2;
        }
        if (dw * dh * 3 > maxsize) { av_frame_unref(d->frame); return AVERROR(ENOMEM); }
        if (!d->sws || d->sws_w != fw || d->sws_h != fh || d->sws_fmt != src->format || d->sws_dw != dw || d->sws_dh != dh) {
            if (d->sws) sws_freeContext(d->sws);
            d->sws = sws_getContext(fw, fh, src->format, dw, dh, AV_PIX_FMT_BGR24,
                                    (dw != fw || dh != fh) ? SWS_BILINEAR : SWS_FAST_BILINEAR, NULL, NULL, NULL);
            d->sws_w = fw; d->sws_h = fh; d->sws_fmt = src->format; d->sws_dw = dw; d->sws_dh = dh;
            if (!d->sws) { av_frame_unref(d->frame); return AVERROR(EINVAL); }
        }
        uint8_t *dst[4] = {out, 0, 0, 0};
        int dstride[4] = {dw * 3, 0, 0, 0};
        sws_scale(d->sws, (const uint8_t *const *)src->data, src->linesize, 0, fh, dst, dstride);
        *w = dw; *h_out = dh;
        produced++;
        av_frame_unref(d->frame);
        t0 = qpc_ms();
        if (t_ms) t_ms[2] += t0 - t2;
    }
    return produced;
}

