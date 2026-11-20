package main

import "fmt"

func main() {
	var i, acc int64
	for i < 100000000 {
		acc = (acc + i*3) % 1000003
		i++
	}
	fmt.Println(acc)
}
