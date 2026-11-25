/* Native GStreamer handles stay on this side of the ABI. The Rust renderer holds
 * a read-only sample mapping until it has consumed the corresponding pixels. */
#include <gst/app/gstappsink.h>
#include <gst/video/video.h>
#include <stdint.h>
#include <string.h>

typedef struct {
    GstElement *playbin;
    GstAppSink *sink;
    GstBus *bus;
    gboolean playing, ended, failed, buffering;
    gint64 position, duration, next_query;
    gchar error[1024];
    gchar decoder[128], decoder_device[256];
    gboolean hardware_decoder;
} CinemaMedia;

typedef struct {
    GstSample *sample;
    GstVideoFrame video;
} CinemaSample;

typedef struct {
    CinemaSample *sample;
    const uint8_t *pixels;
    uint32_t width, height;
    int32_t stride;
    double display_aspect;
    double position, duration;
    int32_t playing, ended;
    const char *error;
    const char *decoder;
    const char *decoder_device;
    int32_t hardware_decoder;
} CinemaUpdate;

int cinema_gst_init(char *message, size_t size) {
    GError *error = NULL;
    if (gst_init_check(NULL, NULL, &error))
        return 1;
    g_strlcpy(message, error ? error->message : "GStreamer initialization failed", size);
    g_clear_error(&error);
    return 0;
}

void cinema_media_free(CinemaMedia *m) {
    if (!m)
        return;
    if (m->playbin) {
        /* A paused appsink retains its preroll sample. Flush it before the
         * V4L2 decoder deactivates its pool, or close waits on that buffer. */
        gst_element_send_event(m->playbin, gst_event_new_flush_start());
        gst_element_set_state(m->playbin, GST_STATE_NULL);
    }
    gst_clear_object(&m->bus);
    gst_clear_object(&m->playbin);
    gst_clear_object(&m->sink);
    g_free(m);
}

CinemaMedia *cinema_media_open(const char *filename, double volume, char *message, size_t size) {
    GError *error = NULL;
    gchar *uri = gst_filename_to_uri(filename, &error);
    if (!uri) {
        g_strlcpy(message, error ? error->message : "Invalid local file path", size);
        g_clear_error(&error);
        return NULL;
    }
    CinemaMedia *m = g_new0(CinemaMedia, 1);
    m->duration = -1;
    m->playbin = gst_element_factory_make("playbin", NULL);
    m->sink = GST_APP_SINK(gst_element_factory_make("appsink", NULL));
    if (!m->playbin || !m->sink) {
        g_strlcpy(message, "GStreamer needs the playbin and appsink plugins", size);
        goto fail;
    }
    gst_object_ref_sink(m->playbin);
    gst_object_ref_sink(m->sink);
    GstCaps *caps = gst_caps_new_simple("video/x-raw", "format", G_TYPE_STRING, "RGBA", NULL);
    /* GStreamer's clock schedules displayed video against audio. A stalled HMD
     * never accumulates a backlog of full-resolution decoded frames. */
    g_object_set(m->sink, "caps", caps, "max-buffers", 1u, "drop", TRUE,
                 "sync", TRUE, "wait-on-eos", FALSE, "enable-last-sample", FALSE, NULL);
    gst_caps_unref(caps);
    g_object_set(m->playbin, "uri", uri, "video-sink", m->sink, "volume", volume, NULL);
    const gchar *audio_name = g_getenv("XR_CINEMA_AUDIO_SINK");
    if (audio_name && *audio_name) {
        GstElement *audio = gst_element_factory_make(audio_name, NULL);
        if (!audio) {
            g_snprintf(message, size, "Audio sink plugin unavailable: %s", audio_name);
            goto fail;
        }
        gst_object_ref_sink(audio);
        /* Some non-base sinks choose their own internal audio sink. */
        if (g_object_class_find_property(G_OBJECT_GET_CLASS(audio), "sync"))
            g_object_set(audio, "sync", TRUE, NULL);
        g_object_set(m->playbin, "audio-sink", audio, NULL);
        gst_object_unref(audio);
    }
    m->bus = gst_element_get_bus(m->playbin);
    if (gst_element_set_state(m->playbin, GST_STATE_PLAYING) == GST_STATE_CHANGE_FAILURE) {
        g_strlcpy(message, "GStreamer could not start playback", size);
        goto fail;
    }
    m->playing = TRUE;
    g_free(uri);
    return m;
fail:
    g_free(uri);
    cinema_media_free(m);
    return NULL;
}

int cinema_media_seek(CinemaMedia *m, int64_t position) {
    if (m->failed)
        return 0;
    if (m->duration >= 0)
        position = MIN(position, m->duration);
    if (!gst_element_seek_simple(m->playbin, GST_FORMAT_TIME,
                                 GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_ACCURATE, position))
        return 0;
    m->ended = FALSE;
    m->position = position;
    m->next_query = 0;
    return 1;
}

int cinema_media_set_playing(CinemaMedia *m, int playing) {
    if (m->failed)
        return 0;
    if (playing && m->ended && !cinema_media_seek(m, 0))
        return 0;
    GstState state = playing && !m->buffering ? GST_STATE_PLAYING : GST_STATE_PAUSED;
    if (gst_element_set_state(m->playbin, state) == GST_STATE_CHANGE_FAILURE)
        return 0;
    m->playing = playing;
    m->next_query = 0;
    return 1;
}

void cinema_media_set_volume(CinemaMedia *m, double volume) {
    g_object_set(m->playbin, "volume", volume, NULL);
}

void cinema_sample_free(CinemaSample *frame) {
    gst_video_frame_unmap(&frame->video);
    gst_sample_unref(frame->sample);
    g_free(frame);
}

static void fail_media(CinemaMedia *m, CinemaUpdate *out, const char *reason) {
    g_strlcpy(m->error, reason, sizeof(m->error));
    out->error = m->error;
    m->playing = FALSE;
    m->failed = TRUE;
    gst_element_set_state(m->playbin, GST_STATE_NULL);
}

static void handle_bus(CinemaMedia *m, CinemaUpdate *out) {
    GstMessage *message;
    while (!m->failed && (message = gst_bus_pop(m->bus))) {
        switch (GST_MESSAGE_TYPE(message)) {
        case GST_MESSAGE_ERROR: {
            GError *error = NULL;
            gchar *debug = NULL;
            gst_message_parse_error(message, &error, &debug);
            g_printerr("XR_CINEMA_MEDIA_ERROR source=%s message=%s debug=%s\n",
                       GST_OBJECT_NAME(GST_MESSAGE_SRC(message)),
                       error ? error->message : "unknown", debug ? debug : "none");
            fail_media(m, out, error ? error->message : "GStreamer playback error");
            g_free(debug);
            g_clear_error(&error);
            break;
        }
        case GST_MESSAGE_EOS:
            m->playing = FALSE;
            m->ended = TRUE;
            gst_element_set_state(m->playbin, GST_STATE_PAUSED);
            m->next_query = 0;
            break;
        case GST_MESSAGE_BUFFERING: {
            gint percent = 0;
            gst_message_parse_buffering(message, &percent);
            m->buffering = percent < 100;
            if (m->playing)
                gst_element_set_state(m->playbin, m->buffering ? GST_STATE_PAUSED : GST_STATE_PLAYING);
            break;
        }
        case GST_MESSAGE_CLOCK_LOST:
            if (m->playing && !m->buffering) {
                gst_element_set_state(m->playbin, GST_STATE_PAUSED);
                gst_element_set_state(m->playbin, GST_STATE_PLAYING);
            }
            break;
        case GST_MESSAGE_DURATION_CHANGED:
            m->duration = -1;
            m->next_query = 0;
            break;
        case GST_MESSAGE_ASYNC_DONE:
            if (GST_MESSAGE_SRC(message) == GST_OBJECT(m->playbin)) {
                gint videos = 0;
                g_object_get(m->playbin, "n-video", &videos, NULL);
                if (videos == 0)
                    fail_media(m, out, "The file has no decodable video stream");
                m->next_query = 0;
            }
            break;
        default:
            break;
        }
        gst_message_unref(message);
    }
}

static void pull_frame(CinemaMedia *m, CinemaUpdate *out) {
    /* try_pull_sample clears appsink's preroll reference even when its queue
     * is empty. Read the preroll directly after a paused seek. */
    GstSample *sample = !m->playing && !m->ended
        ? gst_app_sink_try_pull_preroll(m->sink, 0)
        : gst_app_sink_try_pull_sample(m->sink, 0);
    if (!sample)
        return;
    GstVideoInfo info;
    CinemaSample *frame = g_new0(CinemaSample, 1);
    GstCaps *caps = gst_sample_get_caps(sample);
    GstBuffer *buffer = gst_sample_get_buffer(sample);
    if (!caps || !buffer || !gst_video_info_from_caps(&info, caps) ||
        GST_VIDEO_INFO_FORMAT(&info) != GST_VIDEO_FORMAT_RGBA ||
        !gst_video_frame_map(&frame->video, &info, buffer, GST_MAP_READ)) {
        g_free(frame);
        gst_sample_unref(sample);
        fail_media(m, out, "Unable to map decoded RGBA frame");
        return;
    }
    frame->sample = sample;
    out->sample = frame;
    out->pixels = GST_VIDEO_FRAME_PLANE_DATA(&frame->video, 0);
    out->width = GST_VIDEO_FRAME_WIDTH(&frame->video);
    out->height = GST_VIDEO_FRAME_HEIGHT(&frame->video);
    out->stride = GST_VIDEO_FRAME_PLANE_STRIDE(&frame->video, 0);
    out->display_aspect = (double)out->width / out->height;
    if (info.par_n > 0 && info.par_d > 0)
        out->display_aspect *= (double)info.par_n / info.par_d;
}

static void describe_decoder(CinemaMedia *m) {
    GstIterator *iterator = gst_bin_iterate_recurse(GST_BIN(m->playbin));
    GValue item = G_VALUE_INIT;
    gboolean done = FALSE;
    while (!done) {
        switch (gst_iterator_next(iterator, &item)) {
        case GST_ITERATOR_OK: {
            GstElement *element = g_value_get_object(&item);
            GstElementFactory *factory = gst_element_get_factory(element);
            const gchar *klass = factory ? gst_element_factory_get_metadata(
                factory, GST_ELEMENT_METADATA_KLASS) : NULL;
            if (klass && strstr(klass, "Decoder") && strstr(klass, "Video")) {
                g_strlcpy(m->decoder, gst_plugin_feature_get_name(GST_PLUGIN_FEATURE(factory)),
                          sizeof(m->decoder));
                m->hardware_decoder = strstr(klass, "Hardware") != NULL;
                GParamSpec *device = g_object_class_find_property(G_OBJECT_GET_CLASS(element), "device");
                if (device && (device->flags & G_PARAM_READABLE) &&
                    G_PARAM_SPEC_VALUE_TYPE(device) == G_TYPE_STRING) {
                    gchar *path = NULL;
                    g_object_get(element, "device", &path, NULL);
                    g_strlcpy(m->decoder_device, path ? path : "", sizeof(m->decoder_device));
                    g_free(path);
                }
                g_printerr("XR_CINEMA_DECODER factory=%s hardware=%d device=%s\n",
                           m->decoder, m->hardware_decoder, m->decoder_device);
                done = TRUE;
            }
            g_value_reset(&item);
            break;
        }
        case GST_ITERATOR_RESYNC:
            gst_iterator_resync(iterator);
            break;
        default:
            done = TRUE;
            break;
        }
    }
    if (G_VALUE_TYPE(&item))
        g_value_unset(&item);
    gst_iterator_free(iterator);
}

void cinema_media_poll(CinemaMedia *m, CinemaUpdate *out) {
    memset(out, 0, sizeof(*out));
    handle_bus(m, out);
    if (!m->failed)
        pull_frame(m, out);
    if (out->sample && !m->decoder[0])
        describe_decoder(m);
    out->decoder = m->decoder[0] ? m->decoder : NULL;
    out->decoder_device = m->decoder_device[0] ? m->decoder_device : NULL;
    out->hardware_decoder = m->hardware_decoder;
    gint64 now = g_get_monotonic_time();
    if (!m->failed && now >= m->next_query) {
        gint64 ns;
        if (gst_element_query_position(m->playbin, GST_FORMAT_TIME, &ns) && ns >= 0)
            m->position = ns;
        if (m->duration < 0 && gst_element_query_duration(m->playbin, GST_FORMAT_TIME, &ns) && ns >= 0)
            m->duration = ns;
        if (m->ended && m->duration >= 0)
            m->position = m->duration;
        m->next_query = now + 100 * G_TIME_SPAN_MILLISECOND;
    }
    out->position = (double)m->position / GST_SECOND;
    out->duration = (double)m->duration / GST_SECOND;
    out->playing = m->playing;
    out->ended = m->ended;
}
