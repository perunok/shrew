" neovim init.vim
let g:mapleader = ','
set ts=8 sw=4 et
set nonumber
set noai
set bg=light
nnoremap <leader>f <cmd>Telescope find_files<cr>
lua << EOF
require('lualine').setup()
vim.keymap.set('n', 'X', 'dd')
EOF
autocmd TextYankPost * silent! lua vim.highlight.on_yank()
