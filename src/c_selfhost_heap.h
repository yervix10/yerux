#include <stddef.h>

typedef void (*ScHeapDrop)(void *);
typedef struct ScHeapCell {
    ScHeapDrop drop;
    struct ScHeapCell *next;
    _Alignas(max_align_t) unsigned char data[];
} *ScBox;

typedef struct ScHeapDropFrame {
    ScHeapDrop drop;
    ScBox pending;
    struct ScHeapDropFrame *previous;
} ScHeapDropFrame;

static _Thread_local ScHeapDropFrame *sc_heap_drop;

static inline ScBox sc_box_new(size_t size, ScHeapDrop drop) {
    ScBox box = malloc(sizeof(*box) + size);
    if (!box) nx_panic("out of memory");
    box->drop = drop;
    box->next = NULL;
    return box;
}

static inline ScBox sc_take_ScBox(ScBox *box) {
    ScBox value = *box;
    *box = NULL;
    return value;
}

static inline void sc_drop_ScBox(ScBox *slot) {
    ScBox box = sc_take_ScBox(slot);
    if (!box || !box->drop) {
        free(box);
        return;
    }
    // A repeated destructor signals a recursive path. Keep its pending
    // cells on a worklist while unrelated destructors run synchronously.
    for (ScHeapDropFrame *frame = sc_heap_drop; frame; frame = frame->previous) {
        if (frame->drop == box->drop) {
            box->next = frame->pending;
            frame->pending = box;
            return;
        }
    }
    ScHeapDropFrame frame = {box->drop, box, sc_heap_drop};
    sc_heap_drop = &frame;
    while (frame.pending) {
        box = frame.pending;
        frame.pending = box->next;
        box->drop(box->data);
        free(box);
    }
    sc_heap_drop = frame.previous;
}

typedef struct { ScBox box; const NxDynSlot *table; } ScDyn;

static inline ScDyn sc_take_ScDyn(ScDyn *slot) {
    ScDyn value = *slot;
    *slot = (ScDyn){0};
    return value;
}

static inline void sc_drop_ScDyn(ScDyn *slot) {
    ScBox box = sc_take_ScDyn(slot).box;
    if (box && box->drop) box->drop(box->data);
    free(box);
}

typedef struct { char unused; } ScThreadDone;
typedef struct { char unused; } ScThreadPanic;
typedef int64_t ScJoin;

static inline ScJoin sc_take_ScJoin(ScJoin *slot) {
    ScJoin handle = *slot;
    *slot = 0;
    return handle;
}

static inline void sc_drop_ScJoin(ScJoin *slot) {
    ScJoin handle = sc_take_ScJoin(slot);
    if (handle) {
        ScBox result = (ScBox)(intptr_t)nx_thread_wait(handle);
        sc_drop_ScBox(&result);
    }
}

typedef struct { NxCleanup slot; bool active, live; } ScWorkerCleanup;

static inline void sc_worker_cleanup(ScWorkerCleanup *cleanup) {
    if (cleanup->active) nx_cleanup_pop();
}

static inline void sc_worker_register(ScWorkerCleanup *cleanup, void *value, ScHeapDrop drop) {
    if (nx_worker_panic) {
        cleanup->active = true;
        cleanup->live = true;
        nx_cleanup_push(&cleanup->slot, &cleanup->live, value, drop);
    }
}

#define SC_WORKER_REGISTER(name, drop) \
    ScWorkerCleanup guard_##name __attribute__((cleanup(sc_worker_cleanup))) = {0}; \
    sc_worker_register(&guard_##name, &name, drop)
