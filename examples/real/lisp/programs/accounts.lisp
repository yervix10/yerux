; Closures as objects: each account is a procedure that answers messages.
(define (make-account balance)
  (define (withdraw amount)
    (if (> amount balance)
        "insufficient funds"
        (begin (set! balance (- balance amount)) balance)))
  (define (deposit amount)
    (set! balance (+ balance amount))
    balance)
  (lambda (msg amount)
    (if (= msg 0)
        (deposit amount)
        (withdraw amount))))

(define ana (make-account 100))
(define ben (make-account 5))
(display "ana deposits 50: ") (display (ana 0 50)) (newline)
(display "ben withdraws 10: ") (display (ben 1 10)) (newline)
(display "ana withdraws 120: ") (display (ana 1 120)) (newline)
(display "ana withdraws 120 again? ") (display (ana 1 30)) (newline)

; a list of accounts, processed with a recursive walk
(define (total accounts)
  (if (null? accounts)
      0
      (+ ((car accounts) 0 0) (total (cdr accounts)))))
(display "total now: ")
(display (total (list ana ben)))
(newline)

; let binds before its body, lambdas capture the frame they were made in
(define rate
  (let ((base 3))
    (lambda (x) (* x base))))
(display (rate 7))
(newline)

; string values
(define (greet who) (display "hello, ") (display who) (newline))
(greet "world")
(greet 'friend)
