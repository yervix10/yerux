package main

import "fmt"

func main() {
	var n int64 = 10000000
	var f []bool
	var i int64
	for i <= n {
		f = append(f, true)
		i++
	}
	f[0] = false
	f[1] = false
	i = 2
	for i*i <= n {
		if f[i] {
			for j := i * i; j <= n; j += i {
				f[j] = false
			}
		}
		i++
	}
	var count, last int64
	for i = 0; i <= n; i++ {
		if f[i] {
			count++
			last = i
		}
	}
	fmt.Println(count)
	fmt.Println(last)
}
