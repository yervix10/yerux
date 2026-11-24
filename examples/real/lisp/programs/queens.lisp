; N-queens by backtracking. placed is a list of columns, most recent row first.

(define (abs-diff a b) (if (< a b) (- b a) (- a b)))

; #t when column col does not share a column or diagonal with the queens in placed.
; dist is the row distance from the new queen to the first element of placed.
(define (safe? col placed dist)
  (if (null? placed)
      #t
      (and (not (= (car placed) col))
           (not (= (abs-diff (car placed) col) dist))
           (safe? col (cdr placed) (+ dist 1)))))

; number of ways to fill rows row..n-1, given the queens already placed
(define (solve n row placed)
  (if (= row n)
      1
      (try-cols n row placed 0)))

(define (try-cols n row placed col)
  (if (= col n)
      0
      (+ (if (safe? col placed 1)
             (solve n (+ row 1) (cons col placed))
             0)
         (try-cols n row placed (+ col 1)))))

(define (queens n) (solve n 0 '()))

(define (report n)
  (display n)
  (display "  ")
  (display (queens n))
  (newline))

(display "N  solutions")
(newline)
(report 4)
(report 5)
(report 6)
(report 7)
(report 8)
