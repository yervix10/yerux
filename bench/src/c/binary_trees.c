#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

typedef struct Tree {
    struct Tree *left, *right;
} Tree;

static Tree *make(int depth) {
    Tree *tree = malloc(sizeof(*tree));
    if (!tree) abort();
    tree->left = depth ? make(depth - 1) : NULL;
    tree->right = depth ? make(depth - 1) : NULL;
    return tree;
}

static int64_t check(const Tree *tree) {
    if (!tree->left) return 1;
    return 1 + check(tree->left) + check(tree->right);
}

static void drop(Tree *tree) {
    if (tree->left) {
        drop(tree->left);
        drop(tree->right);
    }
    free(tree);
}

int main(void) {
    Tree *stretch = make(18);
    printf("%" PRId64 "\n", check(stretch));
    Tree *long_lived = make(17);
    int64_t count = 0;
    for (int depth = 4, iterations = 8192; depth <= 16; depth += 2, iterations /= 4) {
        for (int i = 0; i < iterations; ++i) {
            Tree *tree = make(depth);
            count += check(tree);
            drop(tree);
        }
    }
    printf("%" PRId64 "\n%" PRId64 "\n", count, check(long_lived));
    drop(long_lived);
    drop(stretch);
}
