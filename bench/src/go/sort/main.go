package main

import "fmt"

func pass(src, dst []int64, n, w int64) {
	var lo int64
	for lo < n {
		mid := lo + w
		if mid > n {
			mid = n
		}
		hi := mid + w
		if hi > n {
			hi = n
		}
		i, j, k := lo, mid, lo
		for i < mid && j < hi {
			a := src[i]
			b := src[j]
			if a <= b {
				dst[k] = a
				i++
			} else {
				dst[k] = b
				j++
			}
			k++
		}
		for i < mid {
			dst[k] = src[i]
			i++
			k++
		}
		for j < hi {
			dst[k] = src[j]
			j++
			k++
		}
		lo = lo + w + w
	}
}

func checksum(l []int64) int64 {
	var s, i int64
	for i < int64(len(l)) {
		s += l[i] * (i % 10)
		i++
	}
	return s
}

func main() {
	var n int64 = 200000
	var a, b []int64
	var x, grand int64 = 12345, 0
	for round := 0; round < 10; round++ {
		a, b = a[:0], b[:0]
		for i := int64(0); i < n; i++ {
			x = (x*1103515245 + 12345) % 2147483648
			a = append(a, x)
			b = append(b, 0)
		}
		var w int64 = 1
		flip := 0
		for w < n {
			if flip == 0 {
				pass(a, b, n, w)
				flip = 1
			} else {
				pass(b, a, n, w)
				flip = 0
			}
			w *= 2
		}
		r := a
		if flip != 0 {
			r = b
		}
		grand = (grand + r[0] + r[n-1] + checksum(r)) % 1000000007
	}
	fmt.Println(grand)
}
