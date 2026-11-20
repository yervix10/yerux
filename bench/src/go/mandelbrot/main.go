package main

import "fmt"

func main() {
	var w, h, maxit int64 = 800, 600, 200
	var inside, total int64
	for py := int64(0); py < h; py++ {
		ci := float64(py)*(2.4/600.0) - 1.2
		for px := int64(0); px < w; px++ {
			cr := float64(px)*(3.2/800.0) - 2.2
			zr, zi := 0.0, 0.0
			var it int64
			for it < maxit && zr*zr+zi*zi <= 4.0 {
				t := zr*zr - zi*zi + cr
				zi = 2.0*zr*zi + ci
				zr = t
				it++
			}
			if it == maxit {
				inside++
			}
			total += it
		}
	}
	fmt.Println(inside)
	fmt.Println(total)
}
