// PhoneCam decode and scale shim: H.264 Annex-B access units -> BGR24.
// Modes:
//   0 = cpu (libavcodec h264, 4 slice threads)
//   1 = d3d11va on NVIDIA adapter (DXGI VendorId 0x10DE)
//   2 = cuda / nvdec
//   3 = d3d11va on default adapter (any GPU)
#define COBJMACROS
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <initguid.h>
#include <dxgi.h>
#include <libavcodec/avcodec.h>
#include <libavutil/hwcontext.h>
#include <libavutil/imgutils.h>
#include <libavutil/pixdesc.h>
#include <libswscale/swscale.h>

typedef struct Dec {
    AVCodecContext *ctx;
    AVBufferRef *hwdev;
    enum AVPixelFormat hw_fmt;
    AVFrame *frame, *swframe;
    AVPacket *pkt;
    struct SwsContext *sws;
    int sws_w, sws_h, sws_fmt;
    char name[96];
} Dec;

static int find_nvidia_adapter(void) {
    IDXGIFactory1 *f = NULL;
    int idx = -1;
    if (FAILED(CreateDXGIFactory1(&IID_IDXGIFactory1, (void **)&f))) return -1;
    for (UINT i = 0;; i++) {
        IDXGIAdapter1 *a = NULL;
        if (IDXGIFactory1_EnumAdapters1(f, i, &a) != S_OK) break;
        DXGI_ADAPTER_DESC1 d;
        IDXGIAdapter1_GetDesc1(a, &d);
        IDXGIAdapter1_Release(a);
        if (d.VendorId == 0x10DE && !(d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE)) { idx = (int)i; break; }
    }
    IDXGIFactory1_Release(f);
    return idx;
}

static enum AVPixelFormat get_hw_format(AVCodecContext *c, const enum AVPixelFormat *fmts) {
    Dec *d = (Dec *)c->opaque;
    for (const enum AVPixelFormat *p = fmts; *p != AV_PIX_FMT_NONE; p++)
        if (*p == d->hw_fmt) return *p;
    return AV_PIX_FMT_NONE;
}

void *dec_create(int mode, char *err, int errlen) {
    Dec *d = calloc(1, sizeof(Dec));
    if (!d) {
        if (err && errlen > 0) snprintf(err, errlen, "out of memory");
        return NULL;
    }
    const AVCodec *codec = avcodec_find_decoder(AV_CODEC_ID_H264);
    if (!codec) {
        if (err && errlen > 0) snprintf(err, errlen, "no h264 decoder");
        free(d);
        return NULL;
    }
    d->ctx = avcodec_alloc_context3(codec);
    if (!d->ctx) {
        if (err && errlen > 0) snprintf(err, errlen, "failed to alloc codec context");
        free(d);
        return NULL;
    }
    d->ctx->opaque = d;
    d->ctx->flags |= AV_CODEC_FLAG_LOW_DELAY;
    d->ctx->flags2 |= AV_CODEC_FLAG2_FAST;
    int ret;
    if (mode == 0) {
        d->ctx->thread_count = 4;
        d->ctx->thread_type = FF_THREAD_SLICE;
        snprintf(d->name, sizeof d->name, "cpu (libavcodec h264, 4 slice threads)");
    } else {
        char dev[16] = {0};
        const char *devarg = NULL;
        if (mode == 1) { // D3D11VA NVIDIA
            int a = find_nvidia_adapter();
            if (a < 0) {
                if (err && errlen > 0) snprintf(err, errlen, "no NVIDIA adapter found");
                goto fail;
            }
            snprintf(dev, sizeof dev, "%d", a);
            devarg = dev;
            d->hw_fmt = AV_PIX_FMT_D3D11;
            snprintf(d->name, sizeof d->name, "d3d11va (adapter %d NVIDIA)", a);
            ret = av_hwdevice_ctx_create(&d->hwdev, AV_HWDEVICE_TYPE_D3D11VA, devarg, NULL, 0);
        } else if (mode == 3) { // D3D11VA default/any GPU
            d->hw_fmt = AV_PIX_FMT_D3D11;
            snprintf(d->name, sizeof d->name, "d3d11va (default adapter)");
            ret = av_hwdevice_ctx_create(&d->hwdev, AV_HWDEVICE_TYPE_D3D11VA, NULL, NULL, 0);
        } else if (mode == 2) { // CUDA
            d->hw_fmt = AV_PIX_FMT_CUDA;
            snprintf(d->name, sizeof d->name, "cuda/nvdec");
            ret = av_hwdevice_ctx_create(&d->hwdev, AV_HWDEVICE_TYPE_CUDA, NULL, NULL, 0);
        } else {
            if (err && errlen > 0) snprintf(err, errlen, "unknown mode %d", mode);
            goto fail;
        }

        if (ret < 0) {
            char b[128];
            av_strerror(ret, b, sizeof b);
            if (err && errlen > 0) snprintf(err, errlen, "hwdevice_ctx_create: %s", b);
            goto fail;
        }
        d->ctx->hw_device_ctx = av_buffer_ref(d->hwdev);
        d->ctx->get_format = get_hw_format;
        d->ctx->thread_count = 1;
    }
    ret = avcodec_open2(d->ctx, codec, NULL);
    if (ret < 0) {
        char b[128];
        av_strerror(ret, b, sizeof b);
        if (err && errlen > 0) snprintf(err, errlen, "avcodec_open2: %s", b);
        goto fail;
    }
    d->frame = av_frame_alloc();
    d->swframe = av_frame_alloc();
    d->pkt = av_packet_alloc();
    return d;
fail:
    if (d->ctx) avcodec_free_context(&d->ctx);
    if (d->hwdev) av_buffer_unref(&d->hwdev);
    free(d);
    return NULL;
}

const char *dec_name(void *h) {
    if (!h) return "none";
    return ((Dec *)h)->name;
}

void dec_destroy(void *h) {
    Dec *d = h;
    if (!d) return;
    if (d->sws) sws_freeContext(d->sws);
    av_frame_free(&d->frame);
    av_frame_free(&d->swframe);
    av_packet_free(&d->pkt);
    avcodec_free_context(&d->ctx);
    av_buffer_unref(&d->hwdev);
    free(d);
}

int dec_decode(void *h, const uint8_t *data, int len, uint8_t *out, int maxsize, int *w, int *h_out, int *hw_used) {
    Dec *d = h;
    if (!d) return -1;
    int produced = 0;
    d->pkt->data = (uint8_t *)data;
    d->pkt->size = len;
    int ret = avcodec_send_packet(d->ctx, d->pkt);
    if (ret < 0) return ret;
    for (;;) {
        ret = avcodec_receive_frame(d->ctx, d->frame);
        if (ret == AVERROR(EAGAIN) || ret == AVERROR_EOF) break;
        if (ret < 0) return ret;
        AVFrame *src = d->frame;
        *hw_used = 0;
        if (d->frame->format == d->hw_fmt && d->hw_fmt != 0) {
            av_frame_unref(d->swframe);
            ret = av_hwframe_transfer_data(d->swframe, d->frame, 0);
            if (ret < 0) { av_frame_unref(d->frame); return ret; }
            src = d->swframe;
            *hw_used = 1;
        }
        int fw = src->width, fh = src->height;
        if (fw * fh * 3 > maxsize) { av_frame_unref(d->frame); return AVERROR(ENOMEM); }
        if (!d->sws || d->sws_w != fw || d->sws_h != fh || d->sws_fmt != src->format) {
            if (d->sws) sws_freeContext(d->sws);
            d->sws = sws_getContext(fw, fh, src->format, fw, fh, AV_PIX_FMT_BGR24, SWS_FAST_BILINEAR, NULL, NULL, NULL);
            d->sws_w = fw; d->sws_h = fh; d->sws_fmt = src->format;
            if (!d->sws) { av_frame_unref(d->frame); return AVERROR(EINVAL); }
        }
        uint8_t *dst[4] = {out, 0, 0, 0};
        int dstride[4] = {fw * 3, 0, 0, 0};
        sws_scale(d->sws, (const uint8_t *const *)src->data, src->linesize, 0, fh, dst, dstride);
        *w = fw; *h_out = fh;
        produced++;
        av_frame_unref(d->frame);
    }
    return produced;
}

void *scaler_create(int src_w, int src_h, int dst_w, int dst_h) {
    return sws_getContext(src_w, src_h, AV_PIX_FMT_BGR24, dst_w, dst_h, AV_PIX_FMT_BGR24, SWS_BILINEAR, NULL, NULL, NULL);
}

void scaler_scale(void *s, const uint8_t *src, int src_stride, uint8_t *dst, int dst_stride, int h) {
    if (!s || !src || !dst) return;
    const uint8_t *src_slices[4] = { src, NULL, NULL, NULL };
    int src_strides[4] = { src_stride, 0, 0, 0 };
    uint8_t *dst_slices[4] = { dst, NULL, NULL, NULL };
    int dst_strides[4] = { dst_stride, 0, 0, 0 };
    sws_scale((struct SwsContext *)s, src_slices, src_strides, 0, h, dst_slices, dst_strides);
}

void scaler_destroy(void *s) {
    if (s) sws_freeContext((struct SwsContext *)s);
}
