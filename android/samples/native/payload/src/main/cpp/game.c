/* AndySU native sample: a spinning triangle whose colour depends on the payload version. The
 * rotation angle is the state carried across live swaps. */
#include <GLES2/gl2.h>
#include <android/log.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "andysu_payload.h"

static const andysu_host *host;
static GLuint prog;
static float angle;
static int frames, healthy;

static GLuint shader(GLenum t, const char *src) {
    GLuint s = glCreateShader(t);
    glShaderSource(s, 1, &src, NULL);
    glCompileShader(s);
    return s;
}

static int start(const andysu_host *h, const char *state, size_t len) {
    host = h;
    frames = healthy = 0;
    if (state && len) {
        const char *p = strstr(state, "\"angle\":");
        if (p) angle = (float)atof(p + 8);
    }
    prog = glCreateProgram();
    glAttachShader(prog, shader(GL_VERTEX_SHADER,
        "attribute vec2 p; uniform float a; void main(){ float c=cos(a), s=sin(a);"
        " gl_Position=vec4(c*p.x-s*p.y, s*p.x+c*p.y, 0.0, 1.0); }"));
    glAttachShader(prog, shader(GL_FRAGMENT_SHADER,
        "precision mediump float; uniform vec3 col; void main(){ gl_FragColor=vec4(col,1.0); }"));
    glLinkProgram(prog);
    char ev[200];
    snprintf(ev, sizeof ev, "{\"event\":\"sample_started\",\"sample\":\"native\",\"version\":\"%s\",\"angle\":%.2f}",
             SAMPLE_VERSION, angle);
    h->emit(ev);
    return 0;
}

static int frame(void) {
    float v = (float)atof(SAMPLE_VERSION);
    glViewport(0, 0, host->surface_width, host->surface_height);
    glClearColor(0.1f, 0.1f, 0.15f, 1);
    glClear(GL_COLOR_BUFFER_BIT);
    static const float tri[] = {0, 0.6f, -0.5f, -0.4f, 0.5f, -0.4f};
    glUseProgram(prog);
    GLint loc = glGetAttribLocation(prog, "p");
    glVertexAttribPointer((GLuint)loc, 2, GL_FLOAT, GL_FALSE, 0, tri);
    glEnableVertexAttribArray((GLuint)loc);
    glUniform1f(glGetUniformLocation(prog, "a"), angle);
    glUniform3f(glGetUniformLocation(prog, "col"), fmodf(v * 0.37f, 1), fmodf(v * 0.61f, 1), 0.8f);
    glDrawArrays(GL_TRIANGLES, 0, 3);
    angle += 0.02f;
    if (++frames == 30 && !healthy) {
        healthy = 1;
        host->mark_healthy();
    }
    return 0;
}

static char *suspend(size_t *len) {
    glDeleteProgram(prog); /* release GL objects; the host keeps the context */
    char *s = malloc(64);
    *len = (size_t)snprintf(s, 64, "{\"v\":1,\"angle\":%.4f}", angle);
    return s;
}

ANDYSU_PAYLOAD_FULL(start, suspend, frame)
