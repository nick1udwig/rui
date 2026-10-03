/* All opaque native objects remain on one owning thread. This bridge uses the
 * actual installed headers, including variadic capability APIs and enum ABI. */
#define _GNU_SOURCE
#include <libei.h>
#include <libeis.h>
#include <xkbcommon/xkbcommon.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define TOUCH_LIMIT 128
struct rui_native {
    struct ei *sender;
    struct eis *server;
    struct eis_seat *seat;
    struct eis_device *server_device;
    struct ei_device *sender_device;
    struct xkb_context *xkb_context;
    struct xkb_keymap *keymap;
    struct xkb_state *xkb_state;
    char *keymap_text;
    size_t keymap_length;
    struct ei_touch *touches[TOUCH_LIMIT];
    uint64_t touch_ids[TOUCH_LIMIT];
    uint32_t x, y, width, height;
    double scale;
    uint32_t sequence;
    uint64_t last_frame;
    int ready, failed;
};
struct rui_event {
    uint32_t kind, sequence, code, pressed;
    uint64_t time, touch_id;
    double x, y;
};

void rui_native_destroy(struct rui_native *ctx) {
    if (!ctx) return;
    for (int i=0;i<TOUCH_LIMIT;i++) if (ctx->touches[i]) ei_touch_unref(ctx->touches[i]);
    if (ctx->sender_device) ei_device_unref(ctx->sender_device);
    if (ctx->server_device) eis_device_unref(ctx->server_device);
    if (ctx->seat) eis_seat_unref(ctx->seat);
    if (ctx->sender) ei_unref(ctx->sender);
    if (ctx->server) eis_unref(ctx->server);
    if (ctx->xkb_state) xkb_state_unref(ctx->xkb_state);
    if (ctx->keymap) xkb_keymap_unref(ctx->keymap);
    if (ctx->xkb_context) xkb_context_unref(ctx->xkb_context);
    free(ctx->keymap_text);
    free(ctx);
}

struct rui_native *rui_native_new(uint32_t x, uint32_t y, uint32_t width,
                                 uint32_t height, double scale, const char *layout) {
    struct rui_native *ctx = calloc(1, sizeof(*ctx));
    if (!ctx) return NULL;
    ctx->x=x; ctx->y=y; ctx->width=width; ctx->height=height; ctx->scale=scale;
    ctx->xkb_context = xkb_context_new(XKB_CONTEXT_NO_ENVIRONMENT_NAMES);
    if (!ctx->xkb_context) goto fail;
    struct xkb_rule_names names = {.rules="evdev", .model="pc105", .layout=layout};
    ctx->keymap = xkb_keymap_new_from_names(ctx->xkb_context, &names, XKB_KEYMAP_COMPILE_NO_FLAGS);
    if (!ctx->keymap) goto fail;
    ctx->xkb_state = xkb_state_new(ctx->keymap);
    if (!ctx->xkb_state) goto fail;
    ctx->keymap_text=xkb_keymap_get_as_string(ctx->keymap,XKB_KEYMAP_FORMAT_TEXT_V1);
    if (!ctx->keymap_text) goto fail;
    ctx->keymap_length=strlen(ctx->keymap_text)+1;
    if (ctx->keymap_length>128*1024) goto fail;
    ctx->server = eis_new(NULL);
    if (!ctx->server || eis_setup_backend_fd(ctx->server) != 0) goto fail;
    int fd = eis_backend_fd_add_client(ctx->server);
    if (fd < 0) goto fail;
    ctx->sender = ei_new_sender(NULL);
    if (!ctx->sender) {close(fd); goto fail;}
    ei_configure_name(ctx->sender, "RUI isolated session adapter");
    /* ei_setup_backend_fd takes ownership even on setup failure. */
    if (ei_setup_backend_fd(ctx->sender, fd) != 0) goto fail;
    return ctx;
fail:
    rui_native_destroy(ctx);
    return NULL;
}

static int create_device(struct rui_native *ctx, struct eis_event *event) {
    if (ctx->server_device) return -1;
    const enum eis_device_capability caps[] = {EIS_DEVICE_CAP_POINTER_ABSOLUTE,
        EIS_DEVICE_CAP_KEYBOARD,EIS_DEVICE_CAP_BUTTON,EIS_DEVICE_CAP_SCROLL,EIS_DEVICE_CAP_TOUCH};
    for (size_t i=0;i<sizeof(caps)/sizeof(caps[0]);i++)
        if (!eis_event_seat_has_capability(event,caps[i])) return -1;
    ctx->server_device=eis_seat_new_device(ctx->seat);
    if (!ctx->server_device) return -1;
    eis_device_configure_name(ctx->server_device,"RUI private combined device");
    eis_device_configure_type(ctx->server_device,EIS_DEVICE_TYPE_VIRTUAL);
    for (size_t i=0;i<sizeof(caps)/sizeof(caps[0]);i++) eis_device_configure_capability(ctx->server_device,caps[i]);
    struct eis_region *region=eis_device_new_region(ctx->server_device);
    if (!region) return -1;
    eis_region_set_offset(region,ctx->x,ctx->y);
    eis_region_set_size(region,ctx->width,ctx->height);
    eis_region_set_physical_scale(region,ctx->scale);
    eis_region_set_mapping_id(region,"rui.isolated.canvas.v1");
    eis_region_add(region);
    eis_region_unref(region);
    size_t length=ctx->keymap_length;
    int fd=memfd_create("rui-native-keymap",MFD_CLOEXEC);
    if (fd<0) return -1;
    ssize_t written=write(fd,ctx->keymap_text,length);
    if (written<0 || (size_t)written!=length) {close(fd);return -1;}
    struct eis_keymap *keymap=eis_device_new_keymap(ctx->server_device,EIS_KEYMAP_TYPE_XKB,fd,length);
    close(fd);
    if (!keymap) return -1;
    eis_keymap_add(keymap);
    eis_keymap_unref(keymap);
    eis_device_add(ctx->server_device);
    eis_device_resume(ctx->server_device);
    return 0;
}

/* Returns one owned-by-value receiving event, zero if none, -1 on failure.
 * All native events are unref'd, including unsupported protocol events. */
int rui_native_next(struct rui_native *ctx, struct rui_event *out) {
    if (ctx->failed) return -1;
    ei_dispatch(ctx->sender);
    struct ei_event *client_event;
    int count=0;
    while ((client_event=ei_get_event(ctx->sender))) {
        enum ei_event_type kind=ei_event_get_type(client_event);
        switch (kind) {
        case EI_EVENT_CONNECT: break;
        case EI_EVENT_SEAT_ADDED:
            ei_seat_bind_capabilities(ei_event_get_seat(client_event),EI_DEVICE_CAP_POINTER_ABSOLUTE,
                EI_DEVICE_CAP_KEYBOARD,EI_DEVICE_CAP_BUTTON,EI_DEVICE_CAP_SCROLL,EI_DEVICE_CAP_TOUCH,NULL);
            break;
        case EI_EVENT_DEVICE_ADDED:
            if (ctx->sender_device) ctx->failed=1;
            else ctx->sender_device=ei_device_ref(ei_event_get_device(client_event));
            break;
        case EI_EVENT_DEVICE_RESUMED: ctx->ready=1; break;
        case EI_EVENT_DEVICE_PAUSED: ctx->ready=0; break;
        case EI_EVENT_DISCONNECT: case EI_EVENT_DEVICE_REMOVED: case EI_EVENT_SEAT_REMOVED:
            ctx->failed=1;ctx->ready=0;break;
        default: break;
        }
        ei_event_unref(client_event);
        if (++count>256) {ctx->failed=1;return -1;}
    }
    eis_dispatch(ctx->server);
    struct eis_event *event=eis_get_event(ctx->server);
    if (!event) return ctx->failed ? -1:0;
    memset(out,0,sizeof(*out));
    enum eis_event_type kind=eis_event_get_type(event);
    out->kind=(uint32_t)kind;
    if (kind==EIS_EVENT_FRAME) out->time=eis_event_get_time(event);
    switch (kind) {
    case EIS_EVENT_CLIENT_CONNECT: {
        struct eis_client *client=eis_event_get_client(event);
        if (!eis_client_is_sender(client) || ctx->seat) {eis_client_disconnect(client);ctx->failed=1;break;}
        eis_client_connect(client);
        ctx->seat=eis_client_new_seat(client,"rui-isolated-seat");
        if (!ctx->seat) {ctx->failed=1;break;}
        const enum eis_device_capability caps[]={EIS_DEVICE_CAP_POINTER_ABSOLUTE,EIS_DEVICE_CAP_KEYBOARD,
            EIS_DEVICE_CAP_BUTTON,EIS_DEVICE_CAP_SCROLL,EIS_DEVICE_CAP_TOUCH};
        for (size_t i=0;i<sizeof(caps)/sizeof(caps[0]);i++) eis_seat_configure_capability(ctx->seat,caps[i]);
        eis_seat_add(ctx->seat);
        break;
    }
    case EIS_EVENT_SEAT_BIND: if (create_device(ctx,event)!=0) ctx->failed=1; break;
    case EIS_EVENT_DEVICE_START_EMULATING: ctx->sequence=eis_event_emulating_get_sequence(event); break;
    case EIS_EVENT_POINTER_MOTION_ABSOLUTE:
        out->x=eis_event_pointer_get_absolute_x(event);out->y=eis_event_pointer_get_absolute_y(event);break;
    case EIS_EVENT_BUTTON_BUTTON:
        out->code=eis_event_button_get_button(event);out->pressed=eis_event_button_get_is_press(event);break;
    case EIS_EVENT_SCROLL_DELTA:
        out->x=eis_event_scroll_get_dx(event);out->y=eis_event_scroll_get_dy(event);break;
    case EIS_EVENT_KEYBOARD_KEY:
        out->code=eis_event_keyboard_get_key(event);out->pressed=eis_event_keyboard_get_key_is_press(event);break;
    case EIS_EVENT_TOUCH_DOWN: case EIS_EVENT_TOUCH_MOTION: case EIS_EVENT_TOUCH_UP:
        out->touch_id=eis_event_touch_get_id(event);
        if (kind!=EIS_EVENT_TOUCH_UP) {out->x=eis_event_touch_get_x(event);out->y=eis_event_touch_get_y(event);}
        else out->pressed=eis_event_touch_get_is_cancel(event);
        break;
    case EIS_EVENT_CLIENT_DISCONNECT: case EIS_EVENT_DEVICE_CLOSED: ctx->failed=1;ctx->ready=0;break;
    default: break;
    }
    out->sequence=ctx->sequence;
    eis_event_unref(event);
    return ctx->failed ? -1:1;
}
int rui_native_ready(struct rui_native *ctx) {return ctx->ready && !ctx->failed;}
int rui_native_region(struct rui_native *ctx, uint32_t *region) {
    if (!ctx->sender_device) return -1;
    struct ei_region *r=ei_device_get_region(ctx->sender_device,0);
    if (!r || ei_device_get_region(ctx->sender_device,1)) return -1;
    region[0]=ei_region_get_x(r);region[1]=ei_region_get_y(r);
    region[2]=ei_region_get_width(r);region[3]=ei_region_get_height(r);
    return 0;
}
int rui_native_start(struct rui_native *ctx,uint32_t sequence) {
    if (!rui_native_ready(ctx) || sequence==0) return -1;
    ei_device_start_emulating(ctx->sender_device,sequence);return 0;
}
int rui_native_send(struct rui_native *ctx,uint32_t kind,uint32_t code,uint32_t pressed,
                    uint64_t touch_id,double x,double y) {
    if (!rui_native_ready(ctx)) return -1;
    switch (kind) {
    case EIS_EVENT_POINTER_MOTION_ABSOLUTE: ei_device_pointer_motion_absolute(ctx->sender_device,x,y);break;
    case EIS_EVENT_BUTTON_BUTTON: ei_device_button_button(ctx->sender_device,code,pressed!=0);break;
    case EIS_EVENT_SCROLL_DELTA: ei_device_scroll_delta(ctx->sender_device,x,y);break;
    case EIS_EVENT_KEYBOARD_KEY: ei_device_keyboard_key(ctx->sender_device,code,pressed!=0);break;
    case EIS_EVENT_TOUCH_DOWN: case EIS_EVENT_TOUCH_MOTION: case EIS_EVENT_TOUCH_UP: {
        int index=-1;
        for (int i=0;i<TOUCH_LIMIT;i++) if (ctx->touches[i] && ctx->touch_ids[i]==touch_id) {index=i;break;}
        if (kind==EIS_EVENT_TOUCH_DOWN) {
            if (index>=0) return -1;
            for (int i=0;i<TOUCH_LIMIT;i++) if (!ctx->touches[i]) {index=i;break;}
            if (index<0) return -1;
            ctx->touches[index]=ei_device_touch_new(ctx->sender_device);
            if (!ctx->touches[index]) return -1;
            ctx->touch_ids[index]=touch_id;
            ei_touch_down(ctx->touches[index],x,y);
        } else {
            if (index<0) return -1;
            if (kind==EIS_EVENT_TOUCH_MOTION) ei_touch_motion(ctx->touches[index],x,y);
            else {
                if (pressed) ei_touch_cancel(ctx->touches[index]);else ei_touch_up(ctx->touches[index]);
                ei_touch_unref(ctx->touches[index]);ctx->touches[index]=NULL;
            }
        }
        break;
    }
    default: return -1;
    }
    return 0;
}
uint64_t rui_native_frame(struct rui_native *ctx) {
    uint64_t now=ei_now(ctx->sender);
    if (now<=ctx->last_frame) now=ctx->last_frame+1;
    ctx->last_frame=now;
    ei_device_frame(ctx->sender_device,now);
    return now;
}
void rui_native_stop(struct rui_native *ctx) {ei_device_stop_emulating(ctx->sender_device);}
int rui_native_key(struct rui_native *ctx,uint32_t code,int pressed,int update,char *text,size_t capacity) {
    xkb_keycode_t key=code+8;
    if (!xkb_keymap_key_get_name(ctx->keymap,key)) return -1;
    if (update) xkb_state_update_key(ctx->xkb_state,key,pressed ? XKB_KEY_DOWN:XKB_KEY_UP);
    if (!pressed) return 0;
    return xkb_state_key_get_utf8(ctx->xkb_state,key,text,capacity);
}
int rui_native_repeats(struct rui_native *ctx,uint32_t code) {return xkb_keymap_key_repeats(ctx->keymap,code+8);}
int rui_native_has_key(struct rui_native *ctx,uint32_t code) {return xkb_keymap_key_get_name(ctx->keymap,code+8)!=NULL;}
int rui_native_clear_keys(struct rui_native *ctx) {
    struct xkb_state *state=xkb_state_new(ctx->keymap);
    if (!state) return -1;
    xkb_state_unref(ctx->xkb_state);ctx->xkb_state=state;return 0;
}
const unsigned char *rui_native_keymap(struct rui_native *ctx,size_t *length) {
    *length=ctx->keymap_length;return (const unsigned char *)ctx->keymap_text;
}
