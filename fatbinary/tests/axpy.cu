// taken from https://llvm.org/docs/CompileCudaWithLLVM.html
// https://gist.github.com/anonymous/855e277884eb6b388cd2f00d956c2fd4

#include <iostream>

__global__ void axpy(float a, float* x, float* y) {
  y[threadIdx.x] = a * x[threadIdx.x];
}

int main(int argc, char* argv[]) {
  const int kDataLen = 4;

  float a = 2.0f;
  float host_x[kDataLen] = {1.0f, 2.0f, 3.0f, 4.0f};
  float host_y[kDataLen];

  // Copy input data to device.
  float* device_x;
  float* device_y;
  cudaMalloc(&device_x, kDataLen * sizeof(float));
  cudaMalloc(&device_y, kDataLen * sizeof(float));
  cudaMemcpy(device_x, host_x, kDataLen * sizeof(float),
             cudaMemcpyHostToDevice);

  // Launch the kernel.
  axpy<<<1, kDataLen>>>(a, device_x, device_y);

  // Copy output data to host.
  cudaDeviceSynchronize();
  cudaMemcpy(host_y, device_y, kDataLen * sizeof(float),
             cudaMemcpyDeviceToHost);

  // Print the results.
  for (int i = 0; i < kDataLen; ++i) {
    std::cout << "y[" << i << "] = " << host_y[i] << "\n";
  }

  cudaDeviceReset();
  return 0;
}