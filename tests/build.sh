#!/bin/sh
clang++ axpy.cu -o axpy --cuda-gpu-arch=sm_70 \
    -L/usr/local/cuda/lib64 \
    -lcudart_static -ldl -lrt -pthread --save-temps -v