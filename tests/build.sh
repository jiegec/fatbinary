#!/bin/sh
CFLAGS="--cuda-gpu-arch=sm_70 \
    -L/usr/local/cuda/lib64 \
    -lcudart_static -ldl -lrt -pthread --save-temps -v"
CXX="${CXX:-clang++}"

$CXX axpy.cu -o axpy $CFLAGS
cp axpy.cu-cuda-nvptx64-nvidia-cuda.fatbin axpy-default.fatbin

$CXX axpy.cu -g -o axpy $CFLAGS
cp axpy.cu-cuda-nvptx64-nvidia-cuda.fatbin axpy-debug.fatbin

nvcc axpy.cu -Xptxas -O3 -o axpy-ptxas-options.fatbin --fatbin