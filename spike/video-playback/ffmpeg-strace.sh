#!/usr/bin/env bash
# Pass as --ffmpeg to server.mjs: runs the real ffmpeg ($FFMPEG_REAL) under
# strace and appends every file-related call to $STRACE_LOG (a scratch path).
exec strace -f -qq -e trace=%file -o "${STRACE_LOG:?}.$$" "${FFMPEG_REAL:?}" "$@"
