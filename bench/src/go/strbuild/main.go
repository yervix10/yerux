package main

import "fmt"

func pushInt(s []byte, n int64) []byte {
	var buf [24]byte
	k := 0
	var u uint64
	if n < 0 {
		u = -uint64(n)
	} else {
		u = uint64(n)
	}
	for {
		buf[k] = byte('0' + u%10)
		k++
		u /= 10
		if u == 0 {
			break
		}
	}
	if n < 0 {
		s = append(s, '-')
	}
	for k > 0 {
		k--
		s = append(s, buf[k])
	}
	return s
}

func main() {
	var s []byte
	var i int64
	for i < 8000000 {
		s = append(s, "ab"...)
		s = pushInt(s, i)
		i++
	}
	var sum int64
	for j := 0; j < len(s); j++ {
		sum += int64(s[j])
	}
	fmt.Println(len(s))
	fmt.Println(sum)
}
